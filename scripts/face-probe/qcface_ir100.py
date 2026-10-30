"""QCFace IR100 inference backbone; no loss head or training dependencies.

Adapted from hpcc-hcmut/QCFace commit def92fa64940be939938ee3784553f0144f35166,
model/iresnet.py. See LICENSE.qcface.txt. The published evaluation uses RGB,
mean (0.5312, 0.4265, 0.3753), std (0.2873, 0.2555, 0.2496).
"""

from torch import nn


def batch_norm(channels):
    return nn.BatchNorm2d(channels, eps=2e-5, momentum=0.9)


class BasicBlock(nn.Module):
    def __init__(self, incoming, outgoing, stride):
        super().__init__()
        self.bn1 = batch_norm(incoming)
        self.conv1 = nn.Conv2d(incoming, outgoing, 3, 1, 1, bias=False)
        self.bn2 = batch_norm(outgoing)
        self.prelu = nn.PReLU(outgoing)
        self.conv2 = nn.Conv2d(outgoing, outgoing, 3, stride, 1, bias=False)
        self.bn3 = batch_norm(outgoing)
        self.downsample = None
        if stride != 1 or incoming != outgoing:
            self.downsample = nn.Sequential(
                nn.Conv2d(incoming, outgoing, 1, stride, bias=False), batch_norm(outgoing),
            )

    def forward(self, value):
        residual = self.downsample(value) if self.downsample is not None else value
        result = self.bn3(self.conv2(self.prelu(self.bn2(self.conv1(self.bn1(value))))))
        return result + residual


class QCFaceIR100(nn.Module):
    def __init__(self):
        super().__init__()
        self.conv1 = nn.Conv2d(3, 64, 3, 1, 1, bias=False)
        self.bn1 = batch_norm(64)
        self.prelu = nn.PReLU(64)
        incoming = 64
        for index, (outgoing, count) in enumerate(((64, 3), (128, 13), (256, 30), (512, 3)), 1):
            blocks = [BasicBlock(incoming, outgoing, 2)]
            blocks.extend(BasicBlock(outgoing, outgoing, 1) for _ in range(count - 1))
            setattr(self, f"layer{index}", nn.Sequential(*blocks))
            incoming = outgoing
        self.bn2 = batch_norm(512)
        self.dropout = nn.Dropout2d(0.4)
        self.fc = nn.Linear(512 * 7 * 7, 512)
        self.features = nn.BatchNorm1d(512, eps=2e-5, momentum=0.9)

    def forward(self, value):
        value = self.prelu(self.bn1(self.conv1(value)))
        value = self.layer4(self.layer3(self.layer2(self.layer1(value))))
        return self.features(self.fc(self.dropout(self.bn2(value)).flatten(1)))
