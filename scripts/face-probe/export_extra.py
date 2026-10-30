#!/usr/bin/env python3
"""Export FaceLiVTv2-L or a user-provided QCFace IR100 checkpoint to ONNX.

Only artificial inputs are used for validation. No image or video is opened.
"""

import argparse
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile

from assets import (EXTRA_EXPORTS, EXTRA_EXPORT_VERSION, FACELIVT_SOURCE, MODEL_FILES,
                    ROOT, sha256, verify_asset)


def load_backbone(name, source):
    import torch

    checkpoint = torch.load(source, map_location="cpu", weights_only=True)
    if not isinstance(checkpoint, dict):
        raise ValueError("Checkpoint must contain a tensor state dictionary")
    state = checkpoint.get("state_dict", checkpoint)
    if not isinstance(state, dict) or not state:
        raise ValueError("Empty or invalid state dictionary")
    if any(not isinstance(key, str) or not isinstance(value, torch.Tensor)
           for key, value in state.items()):
        raise ValueError("State dictionary entries must be named tensors")
    state = {key.removeprefix("module."): value for key, value in state.items()}
    if name == "facelivtv2_l":
        from facelivtv2 import FaceLiVTv2L

        factory = FaceLiVTv2L
    else:
        from qcface_ir100 import QCFaceIR100

        factory = QCFaceIR100
        if any(key.startswith("backbone.") for key in state):
            if any(not key.startswith(("backbone.", "head.")) for key in state):
                raise ValueError("Unexpected QCFace checkpoint prefixes")
            state = {key.removeprefix("backbone."): value for key, value in state.items()
                     if key.startswith("backbone.")}
    with torch.device("meta"):
        model = factory()
    model.load_state_dict(state, strict=True, assign=True)
    return model.eval().requires_grad_(False)


def qcface_normalization(backbone):
    """Adapt our common [-1,1] RGB input to QCFace's published RGB mean/std."""
    import torch

    class NormalizedQCFace(torch.nn.Module):
        def __init__(self):
            super().__init__()
            self.backbone = backbone
            self.register_buffer("mean", torch.tensor([0.5312, 0.4265, 0.3753]).view(1, 3, 1, 1))
            self.register_buffer("std", torch.tensor([0.2873, 0.2555, 0.2496]).view(1, 3, 1, 1))

        def forward(self, value):
            return self.backbone(((value + 1) / 2 - self.mean) / self.std)

    return NormalizedQCFace().eval().requires_grad_(False)


def export(root, name, checkpoint=None):
    import numpy as np
    import onnx
    import onnxruntime as ort
    import torch

    target = root / MODEL_FILES[name]
    manifest_path = target.with_suffix(".json")
    if target.exists() or manifest_path.exists():
        raise ValueError(f"{name} export already exists; it will not be overwritten")
    if name == "facelivtv2_l":
        source = root / FACELIVT_SOURCE.path
        verify_asset(source, FACELIVT_SOURCE)
        source_info = {"sourceKind": "pinned", "sourceRepo": FACELIVT_SOURCE.repo,
                       "sourceRevision": FACELIVT_SOURCE.revision,
                       "codeRevision": "d99d86607c7c05540c74e815e5a88847f7e667db"}
    else:
        if checkpoint is None or not checkpoint.is_file():
            raise ValueError("QCFace author download requires Microsoft login. "
                             "Provide --checkpoint /path/to/QCFace-Arc-IR100.pth")
        source = checkpoint
        source_info = {"sourceKind": "user_supplied", "sourceRepo": "hpcc-hcmut/QCFace",
                       "codeRevision": "def92fa64940be939938ee3784553f0144f35166"}
    source_hash = sha256(source)
    torch.set_num_threads(3)
    torch.set_num_interop_threads(1)
    ort.disable_telemetry_events()
    model = load_backbone(name, source)
    if name == "qcface_ir100":
        model = qcface_normalization(model)
    generator = torch.Generator().manual_seed(0)
    sample = torch.rand((1, 3, 112, 112), generator=generator) * 2 - 1
    options = ort.SessionOptions()
    options.intra_op_num_threads = 3
    options.inter_op_num_threads = 1
    target.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="export-", dir=target.parent) as temporary:
        pending = Path(temporary) / "model.onnx"
        with torch.inference_mode():
            torch.onnx.export(model, sample, str(pending), export_params=True,
                              input_names=["input"], output_names=["embedding"],
                              opset_version=17, dynamo=False, external_data=False)
        onnx.checker.check_model(str(pending))
        session = ort.InferenceSession(str(pending), sess_options=options,
                                       providers=["CPUExecutionProvider"])
        errors = []
        with torch.inference_mode():
            for value in (sample, torch.zeros_like(sample), -sample):
                expected = model(value).numpy()
                actual = session.run(None, {"input": value.numpy()})[0]
                if (actual.shape != (1, 512) or not np.isfinite(actual).all()
                        or not np.allclose(actual, expected, rtol=1e-4, atol=1e-4)):
                    raise ValueError(f"{name} ONNX differs from PyTorch on an artificial input")
                errors.append(float(np.max(np.abs(actual - expected))))
        if sha256(source) != source_hash:
            raise ValueError("Source checkpoint changed during export")
        manifest = {
            "model": name, "exportVersion": EXTRA_EXPORT_VERSION, **source_info,
            "sourceSha256": source_hash, "onnxSha256": sha256(pending),
            "torchVersion": torch.__version__, "onnxVersion": onnx.__version__,
            "opset": 17, "inputColor": "RGB", "inputShape": [1, 3, 112, 112],
            "inputNormalization": "rgb_minus_one_to_one",
            "modelNormalization": "published RGB channel mean/std inside graph" if name == "qcface_ir100"
                                  else "RGB mean/std 0.5 outside graph",
            "syntheticValidationMaxAbsError": max(errors), "syntheticValidationInputs": len(errors),
        }
        pending_manifest = Path(temporary) / "model.json"
        pending_manifest.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        if name == "qcface_ir100":
            local_source = target.with_name("model.pt")
            if local_source.exists():
                if sha256(local_source) != source_hash:
                    raise ValueError("Existing QCFace source differs; it will not be overwritten")
            else:
                pending_source = Path(temporary) / "model.pt"
                shutil.copyfile(source, pending_source)
                if sha256(pending_source) != source_hash:
                    raise ValueError("Source checkpoint changed while being copied")
                os.link(pending_source, local_source)
        os.link(pending, target)
        os.link(pending_manifest, manifest_path)
    print(json.dumps(manifest, indent=2), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--model", choices=EXTRA_EXPORTS, required=True)
    parser.add_argument("--checkpoint", type=Path, help="Trusted local QCFace IR100 author checkpoint")
    args = parser.parse_args()
    if args.checkpoint is not None and args.model != "qcface_ir100":
        parser.error("--checkpoint is only accepted for qcface_ir100")
    os.umask(0o077)
    export(args.root.expanduser().resolve(), args.model,
           args.checkpoint.expanduser().resolve() if args.checkpoint else None)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)
    except (OSError, ValueError, RuntimeError, ImportError) as error:
        print(f"Export failed: {error}", file=sys.stderr)
        sys.exit(1)
