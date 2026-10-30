"""Inference-only FaceLiVTv2-L, with the upstream checkpoint's exact key names.

Adapted from backbones/facelivtv2.py at novendrastywn/FaceLiVT commit
d99d86607c7c05540c74e815e5a88847f7e667db. See LICENSE.facelivt.txt.
Only the L configuration and evaluation forward pass are retained. No timm,
training initialization, distillation, or optional deployment reparameterization.
"""

import math

import torch
from torch import nn


class ConvBN(nn.Sequential):
    def __init__(self, incoming, outgoing, kernel=1, stride=1, padding=0, groups=1):
        super().__init__()
        self.add_module("c", nn.Conv2d(incoming, outgoing, kernel, stride, padding,
                                      groups=groups, bias=False))
        self.add_module("bn", nn.BatchNorm2d(outgoing))


class RepConv(nn.Module):
    def __init__(self, incoming, outgoing, kernel=3, stride=1, padding=1, groups=1):
        super().__init__()
        self.conv = nn.Conv2d(incoming, outgoing, kernel, stride, padding, groups=groups)
        self.repconv = nn.Conv2d(incoming, outgoing, kernel // 2, stride,
                                 padding // 2, groups=groups)
        self.bn = nn.BatchNorm2d(outgoing)

    def forward(self, value):
        return self.bn(self.conv(value) + self.repconv(value))


class Residual(nn.Module):
    def __init__(self, module):
        super().__init__()
        self.m = module

    def forward(self, value):
        return value + self.m(value)


class FFN(nn.Module):
    def __init__(self, channels):
        super().__init__()
        self.pw1 = ConvBN(channels, channels * 2)
        self.act = nn.GELU()
        self.pw2 = ConvBN(channels * 2, channels)

    def forward(self, value):
        return self.pw2(self.act(self.pw1(value)))


class Stem(nn.Module):
    def __init__(self):
        super().__init__()
        self.stem = nn.Sequential(
            nn.Sequential(RepConv(3, 32, stride=2), nn.GELU()),
            nn.Sequential(RepConv(32, 64, stride=2), nn.Identity()),
        )

    def forward(self, value):
        return self.stem(value)


class PatchMerging(nn.Module):
    def __init__(self, incoming, outgoing):
        super().__init__()
        self.spatial = nn.Sequential(
            RepConv(incoming, incoming, stride=2, groups=incoming),
            ConvBN(incoming, outgoing),
        )
        self.channel = Residual(FFN(outgoing))

    def forward(self, value):
        return self.channel(self.spatial(value))


class Affine(nn.Module):
    def __init__(self, channels):
        super().__init__()
        self.alpha = nn.Parameter(torch.ones(1, channels, 1) * 1e-5)
        self.beta = nn.Parameter(torch.zeros(1, channels, 1))

    def forward(self, value):
        return torch.addcmul(self.beta, self.alpha, value)


class LinearAttention(nn.Module):
    def __init__(self, channels, resolution):
        super().__init__()
        self.norm = Affine(channels)
        self.lin = nn.ModuleList([nn.Linear(resolution ** 2, resolution ** 2) for _ in range(4)])
        self.ls = nn.Parameter(torch.ones(channels, 1, 1) * 1e-5)

    def forward(self, value):
        batch, channels, height, width = value.shape
        chunks = self.norm(value.reshape(batch, channels, height * width)).chunk(4, dim=1)
        value = torch.cat([layer(chunk) for layer, chunk in zip(self.lin, chunks)], dim=1)
        return self.ls * value.reshape(batch, channels, height, width)


class Block(nn.Module):
    def __init__(self, channels, resolution, attention):
        super().__init__()
        layers = [Residual(RepConv(channels, channels, groups=channels))]
        if attention:
            layers.append(Residual(LinearAttention(channels, resolution)))
        layers.append(Residual(FFN(channels)))
        self.block = nn.Sequential(*layers)

    def forward(self, value):
        return self.block(value)


class Stage(nn.Module):
    def __init__(self, channels, count, resolution, attention):
        super().__init__()
        self.blocks = nn.Sequential(*[Block(channels, resolution, attention) for _ in range(count)])

    def forward(self, value):
        return self.blocks(value)


class EmbeddingHead(nn.Module):
    def __init__(self):
        super().__init__()
        self.classifier = nn.Sequential()
        self.classifier.add_module("bn", nn.BatchNorm1d(1284))
        self.classifier.add_module("l", nn.Linear(1284, 512))

    def forward(self, value):
        return self.classifier(value)


class FaceLiVTv2L(nn.Module):
    def __init__(self):
        super().__init__()
        channels, counts = (64, 128, 256, 512), (3, 3, 9, 3)
        self.patch_embedds = nn.Sequential(Stem(), *[
            PatchMerging(channels[index - 1], channels[index]) for index in range(1, 4)
        ])
        self.stages = nn.Sequential(*[
            Stage(channels[index], counts[index], math.ceil(28 / 2 ** index), index >= 2)
            for index in range(4)
        ])
        self.pre_head = nn.Sequential(ConvBN(512, 1284), ConvBN(1284, 1284, 4, groups=1284))
        self.head = EmbeddingHead()

    def forward(self, value):
        for patch, stage in zip(self.patch_embedds, self.stages):
            value = stage(patch(value))
        return self.head(self.pre_head(value).flatten(1))
