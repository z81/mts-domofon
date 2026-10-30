#!/usr/bin/env python3
"""Convert the pinned CVLFace safetensors to ONNX; validate on artificial tensors."""

import argparse
import json
import os
from pathlib import Path
import tempfile

from assets import ADAFACE_EXPORT_VERSION, ADAFACE_SOURCE, ROOT, sha256, verify_asset


def export(root):
    import numpy as np
    import onnx
    import onnxruntime as ort
    import torch
    from safetensors.torch import load_file

    from adaface_ir101 import IR101

    source = root / ADAFACE_SOURCE.path
    target = source.with_name("model.onnx")
    manifest_path = target.with_suffix(".json")
    if target.exists() or manifest_path.exists():
        raise ValueError("AdaFace export already exists; it will not be overwritten")
    verify_asset(source, ADAFACE_SOURCE)
    torch.set_num_threads(3)
    torch.set_num_interop_threads(1)
    ort.disable_telemetry_events()
    state = load_file(str(source), device="cpu")
    prefix = "model.net."
    if not state or any(not key.startswith(prefix) for key in state):
        raise ValueError("Unexpected CVLFace checkpoint keys")
    with torch.device("meta"):
        model = IR101()
    model.load_state_dict({key[len(prefix):]: value for key, value in state.items()},
                          strict=True, assign=True)
    del state
    model.eval().requires_grad_(False)

    # This checkpoint's published config expects RGB, normalized with mean/std 0.5.
    generator = torch.Generator().manual_seed(0)
    sample = torch.rand((1, 3, 112, 112), generator=generator) * 2 - 1
    options = ort.SessionOptions()
    options.intra_op_num_threads = 3
    options.inter_op_num_threads = 1
    with tempfile.TemporaryDirectory(prefix="export-", dir=source.parent) as temporary:
        pending = Path(temporary) / "model.onnx"
        with torch.inference_mode():
            torch.onnx.export(model, sample, str(pending), export_params=True,
                              input_names=["input"], output_names=["embedding"],
                              opset_version=17, dynamo=False, external_data=False)
            expected = model(sample).numpy()
        onnx.checker.check_model(str(pending))
        session = ort.InferenceSession(str(pending), sess_options=options,
                                       providers=["CPUExecutionProvider"])
        actual = session.run(None, {"input": sample.numpy()})[0]
        if (actual.shape != (1, 512) or not np.isfinite(actual).all()
                or not np.allclose(actual, expected, rtol=1e-4, atol=1e-4)):
            raise ValueError("AdaFace ONNX differs from PyTorch on the synthetic input")
        manifest = {
            "exportVersion": ADAFACE_EXPORT_VERSION,
            "sourceRepo": ADAFACE_SOURCE.repo, "sourceRevision": ADAFACE_SOURCE.revision,
            "sourceSha256": ADAFACE_SOURCE.sha256, "onnxSha256": sha256(pending),
            "torchVersion": torch.__version__, "onnxVersion": onnx.__version__,
            "opset": 17, "inputColor": "RGB", "inputShape": [1, 3, 112, 112],
            "normalization": "(RGB / 255 - 0.5) / 0.5, performed outside the graph",
            "syntheticValidationMaxAbsError": float(np.max(np.abs(actual - expected))),
        }
        pending_manifest = Path(temporary) / "model.json"
        pending_manifest.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        # No overwrite, even if another preparation process wins the race.
        os.link(pending, target)
        os.link(pending_manifest, manifest_path)
    print(json.dumps(manifest, indent=2), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args()
    os.umask(0o077)
    export(args.root.expanduser().resolve())
