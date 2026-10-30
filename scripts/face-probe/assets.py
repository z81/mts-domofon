"""Pinned, public model artifacts. Importing this module does not access the network."""

from dataclasses import dataclass
import hashlib
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parent / "models"


@dataclass(frozen=True)
class Asset:
    path: str
    repo: str
    revision: str
    remote_path: str
    sha256: str
    size: int

    @property
    def url(self):
        return f"https://huggingface.co/{self.repo}/resolve/{self.revision}/{self.remote_path}"


ASSETS = (
    Asset("buffalo_l/det_10g.onnx", "immich-app/buffalo_l",
          "d09715916a0778919a770c343533641e250b8699", "detection/model.onnx",
          "5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91", 16923827),
    Asset("buffalo_l/w600k_r50.onnx", "immich-app/buffalo_l",
          "d09715916a0778919a770c343533641e250b8699", "recognition/model.onnx",
          "4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43", 174383860),
    Asset("antelopev2/glintr100.onnx", "immich-app/antelopev2",
          "51ca7e153bccebdbae224779e23c9c243e2fec85", "recognition/model.onnx",
          "4ab1d6435d639628a6f3e5008dd4f929edf4c4124b1a7169e1048f9fef534cdf", 260665334),
    Asset("adaface_ir101/model.safetensors", "minchul/cvlface_adaface_ir101_webface12m",
          "54f602a0737bd1ee4a4e7e9fd089a485f397fefd", "model.safetensors",
          "2ea535a43877bd3de8091903935c783ce335be66a9f8917fae9a7a18ae4bbf56", 260980552),
    Asset("lvface_b/model.onnx", "bytedance-research/LVFace",
          "b12702ab1f5c721748e054a66dc90e1edd1f0724", "LVFace-B_Glint360K/LVFace-B_Glint360K.onnx",
          "9d834ed8e927fd35b9123b2bf97c40aad05785b1f9ecfb1c4c1f6242d38d1382", 455533594),
    Asset("facelivtv2_l/model.pt", "novendrastywn/FaceLiVT",
          "cec8aee5341fb4fede121a960a418b9e6163cc5c", "facelivtv2-l.pt",
          "068f991b85d8ee27bde231991f5052377d1b078f2d97d38cd3b8e8132426c36e", 35881818),
)

ADAFACE_SOURCE = ASSETS[3]
FACELIVT_SOURCE = ASSETS[5]
BASE_MODELS = ("buffalo_l", "antelopev2", "adaface_ir101")
EXTRA_EXPORTS = ("facelivtv2_l", "qcface_ir100")
EXTRA_EXPORT_VERSION = 1

MODEL_FILES = {
    "buffalo_l": "buffalo_l/w600k_r50.onnx",
    "antelopev2": "antelopev2/glintr100.onnx",
    "adaface_ir101": "adaface_ir101/model.onnx",
    "lvface_b": "lvface_b/model.onnx",
    "facelivtv2_l": "facelivtv2_l/model.onnx",
    "qcface_ir100": "qcface_ir100/model.onnx",
}
ADAFACE_EXPORT_VERSION = 1


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def verify_asset(path, asset):
    if not path.is_file():
        raise ValueError(f"Missing model: {path}. Run prepare_models.py first.")
    if path.stat().st_size != asset.size or sha256(path) != asset.sha256:
        raise ValueError(f"Model checksum mismatch: {path}; existing files are not overwritten")


def verify_adaface_export(root):
    path = root / MODEL_FILES["adaface_ir101"]
    manifest_path = path.with_suffix(".json")
    if not path.is_file() or not manifest_path.is_file():
        raise ValueError("AdaFace ONNX export is missing; run prepare_models.py first")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if (manifest.get("exportVersion") != ADAFACE_EXPORT_VERSION
            or manifest.get("sourceSha256") != ADAFACE_SOURCE.sha256
            or manifest.get("onnxSha256") != sha256(path)
            or manifest.get("inputColor") != "RGB"):
        raise ValueError("AdaFace export manifest/checksum mismatch; move the old export aside")


def verify_extra_export(root, name):
    path = root / MODEL_FILES[name]
    manifest_path = path.with_suffix(".json")
    if not path.is_file() or not manifest_path.is_file():
        raise ValueError(f"{name} ONNX export is missing; run prepare_models.py first")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if name == "facelivtv2_l":
        expected_source = FACELIVT_SOURCE.sha256
    else:
        source = path.with_name("model.pt")
        if not source.is_file() or manifest.get("sourceKind") != "user_supplied":
            raise ValueError("QCFace needs a local author checkpoint; see README.md")
        expected_source = sha256(source)
    if (manifest.get("model") != name
            or manifest.get("exportVersion") != EXTRA_EXPORT_VERSION
            or manifest.get("sourceSha256") != expected_source
            or manifest.get("onnxSha256") != sha256(path)
            or manifest.get("inputColor") != "RGB"
            or manifest.get("inputShape") != [1, 3, 112, 112]
            or manifest.get("inputNormalization") != "rgb_minus_one_to_one"):
        raise ValueError(f"{name} export manifest/checksum mismatch; move the old export aside")


def verify_models(root, names):
    required = {ASSETS[0].path}
    required.update(MODEL_FILES[name] for name in names if name != "adaface_ir101")
    for asset in ASSETS:
        if asset.path in required:
            verify_asset(root / asset.path, asset)
    if "adaface_ir101" in names:
        verify_adaface_export(root)
    for name in EXTRA_EXPORTS:
        if name in names:
            verify_extra_export(root, name)
