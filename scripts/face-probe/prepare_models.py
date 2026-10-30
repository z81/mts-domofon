#!/usr/bin/env python3
"""Prepare selected local models. Never reads photos or video."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from urllib.request import urlopen

from assets import (ADAFACE_SOURCE, ASSETS, BASE_MODELS, EXTRA_EXPORTS, FACELIVT_SOURCE,
                    MODEL_FILES, ROOT, verify_adaface_export, verify_asset, verify_extra_export,
                    verify_models)


def prepare_asset(root, asset, reuse_dirs):
    target = root / asset.path
    if target.exists():
        verify_asset(target, asset)
        print(f"Verified {asset.path}", flush=True)
        return
    target.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    reusable = None
    for directory in reuse_dirs:
        candidate = directory / target.name
        if candidate.is_file():
            try:
                verify_asset(candidate, asset)
            except ValueError:
                continue
            reusable = candidate
            break
    with tempfile.TemporaryDirectory(prefix="download-", dir=target.parent) as temporary:
        pending = Path(temporary) / target.name
        if reusable:
            print(f"Reusing {reusable.name}", flush=True)
            shutil.copyfile(reusable, pending)
        else:
            print(f"Downloading {asset.path} ({asset.size / 1_000_000:.1f} MB)", flush=True)
            with urlopen(asset.url, timeout=60) as response, pending.open("xb") as output:
                shutil.copyfileobj(response, output, length=1024 * 1024)
        verify_asset(pending, asset)
        os.link(pending, target)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--reuse", type=Path, action="append", default=[],
                        help="Reuse verified weights from an existing flat directory")
    parser.add_argument("--model-set", nargs="+", choices=list(MODEL_FILES), default=list(BASE_MODELS))
    parser.add_argument("--qcface-checkpoint", type=Path,
                        help="Trusted local author checkpoint when selecting qcface_ir100")
    args = parser.parse_args()
    if len(args.model_set) != len(set(args.model_set)):
        parser.error("--model-set must not contain duplicates")
    if args.qcface_checkpoint and "qcface_ir100" not in args.model_set:
        parser.error("--qcface-checkpoint requires --model-set qcface_ir100")
    root = args.root.expanduser().resolve()
    os.umask(0o077)
    required = {ASSETS[0].path}
    required.update(MODEL_FILES[name] for name in args.model_set)
    if "adaface_ir101" in args.model_set:
        required.add(ADAFACE_SOURCE.path)
    if "facelivtv2_l" in args.model_set:
        required.add(FACELIVT_SOURCE.path)
    for asset in ASSETS:
        if asset.path in required:
            prepare_asset(root, asset, [path.expanduser().resolve() for path in args.reuse])
    for name in args.model_set:
        if name != "adaface_ir101" and name not in EXTRA_EXPORTS:
            continue
        export_path = root / MODEL_FILES[name]
        if export_path.exists() or export_path.with_suffix(".json").exists():
            if name == "adaface_ir101":
                verify_adaface_export(root)
            else:
                verify_extra_export(root, name)
            if name == "qcface_ir100" and args.qcface_checkpoint:
                from assets import sha256

                supplied = args.qcface_checkpoint.expanduser().resolve()
                if sha256(supplied) != sha256(export_path.with_name("model.pt")):
                    raise ValueError("Provided QCFace checkpoint differs from the existing export")
            print(f"Verified {name} ONNX export", flush=True)
        else:
            print(f"Exporting {name} to ONNX (synthetic tensors only)", flush=True)
            script = "export_adaface.py" if name == "adaface_ir101" else "export_extra.py"
            command = [sys.executable, str(Path(__file__).with_name(script)), "--root", str(root)]
            if name != "adaface_ir101":
                command.extend(["--model", name])
            if name == "qcface_ir100" and args.qcface_checkpoint:
                command.extend(["--checkpoint", str(args.qcface_checkpoint.expanduser().resolve())])
            subprocess.run(command, check=True)
    verify_models(root, args.model_set)
    print(f"Ready: {', '.join(args.model_set)} in {root}", flush=True)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("Preparation interrupted; completed model files have been preserved.", file=sys.stderr)
        sys.exit(130)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Preparation failed: {error}", file=sys.stderr)
        sys.exit(1)
