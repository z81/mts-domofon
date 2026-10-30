"""Inference-only CVLFace IR101 (112px, 512 outputs).

Adapted from Minchul Kim's MIT-licensed CVLFace implementation; see
LICENSE.cvlface.txt. Only the IR101 branch is retained (3, 13, 30, 3 IR blocks).
Source: https://huggingface.co/minchul/cvlface_adaface_ir101_webface12m/blob/
54f602a0737bd1ee4a4e7e9fd089a485f397fefd/models/iresnet/model.py
"""

from torch import nn


class BasicBlockIR(nn.Module):
    def __init__(self, in_channels, channels, stride):
        super().__init__()
        if in_channels == channels:
            self.shortcut_layer = nn.MaxPool2d(1, stride)
        else:
            self.shortcut_layer = nn.Sequential(
                nn.Conv2d(in_channels, channels, 1, stride, bias=False),
                nn.BatchNorm2d(channels),
            )
        self.res_layer = nn.Sequential(
            nn.BatchNorm2d(in_channels),
            nn.Conv2d(in_channels, channels, 3, 1, 1, bias=False),
            nn.BatchNorm2d(channels),
            nn.PReLU(channels),
            nn.Conv2d(channels, channels, 3, stride, 1, bias=False),
            nn.BatchNorm2d(channels),
        )

    def forward(self, value):
        return self.res_layer(value) + self.shortcut_layer(value)


class IR101(nn.Module):
    def __init__(self):
        super().__init__()
        self.input_layer = nn.Sequential(
            nn.Conv2d(3, 64, 3, 1, 1, bias=False),
            nn.BatchNorm2d(64),
            nn.PReLU(64),
        )
        self.output_layer = nn.Sequential(
            nn.BatchNorm2d(512), nn.Dropout(0.4), nn.Flatten(),
            nn.Linear(512 * 7 * 7, 512), nn.BatchNorm1d(512, affine=False),
        )
        layers = []
        in_channels = 64
        for channels, count in ((64, 3), (128, 13), (256, 30), (512, 3)):
            for index in range(count):
                layers.append(BasicBlockIR(in_channels, channels, 2 if index == 0 else 1))
                in_channels = channels
        self.body = nn.Sequential(*layers)

    def forward(self, value):
        return self.output_layer(self.body(self.input_layer(value)))
