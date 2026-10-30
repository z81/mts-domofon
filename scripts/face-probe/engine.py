"""Local SCRFD detection and ArcFace-compatible embeddings; no camera or API I/O."""

import hashlib
import os

os.environ["ORT_DISABLE_TELEMETRY"] = "1"

import cv2
import numpy as np
import onnxruntime as ort
from insightface.app.common import Face
from insightface.model_zoo import get_model
from PIL import Image, ImageOps

from reference_inputs import reference_files


def model_info(path):
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    return {"file": path.name, "bytes": path.stat().st_size, "sha256": digest}


class FaceEngine:
    def __init__(self, detector_path, recognizer_path, *, det_size, threads,
                 min_face, min_eyes):
        ort.disable_telemetry_events()
        cv2.setNumThreads(1)
        options = ort.SessionOptions()
        options.intra_op_num_threads = threads
        options.inter_op_num_threads = 1
        runtime = {"providers": ["CPUExecutionProvider"], "sess_options": options}
        self.detector = get_model(str(detector_path), download=False, **runtime)
        if getattr(self.detector, "taskname", None) != "detection":
            raise ValueError("The detector must be a SCRFD-compatible ONNX model")
        self.detector.prepare(ctx_id=-1, input_size=(det_size, det_size), det_thresh=0.6)
        self.recognizer = None
        self.models = {"detector": model_info(detector_path)}
        if recognizer_path is not None:
            self.recognizer = get_model(str(recognizer_path), download=False, **runtime)
            if getattr(self.recognizer, "taskname", None) != "recognition":
                raise ValueError("The recognizer must be an ArcFace-compatible ONNX model")
            self.recognizer.prepare(ctx_id=-1)
            self.models["recognizer"] = model_info(recognizer_path)
        self.min_face = min_face
        self.min_eyes = min_eyes
        self.references = []

    def detect(self, image, *, input_size=None):
        boxes, landmarks = self.detector.detect(image, input_size=input_size)
        if len(boxes) and (landmarks is None or len(landmarks) != len(boxes)):
            raise ValueError("The detector must return five face landmarks")
        return list(zip(boxes, landmarks)) if len(boxes) else []

    def measure(self, image, box, landmarks):
        if landmarks.shape != (5, 2) or not np.isfinite(landmarks).all():
            raise ValueError("Invalid face landmarks")
        if not np.isfinite(box).all():
            raise ValueError("Invalid face bounding box")
        height, width = image.shape[:2]
        x1, y1, x2, y2 = (float(value) for value in box[:4])
        left, top = max(0, int(x1)), max(0, int(y1))
        right, bottom = min(width, int(np.ceil(x2))), min(height, int(np.ceil(y2)))
        crop = image[top:bottom, left:right] if right > left and bottom > top else None
        eyes = float(np.linalg.norm(landmarks[0] - landmarks[1]))
        reasons = []
        if min(x2 - x1, y2 - y1) < self.min_face:
            reasons.append("face_too_small")
        if eyes < self.min_eyes:
            reasons.append("eyes_too_close_or_profile")
        if x1 < 0 or y1 < 0 or x2 > width or y2 > height:
            reasons.append("face_clipped")
        if crop is None or not crop.size:
            reasons.append("empty_crop")
            gray = None
        else:
            gray = cv2.cvtColor(crop, cv2.COLOR_BGR2GRAY)
        return {
            "box": [round(value, 1) for value in (x1, y1, x2, y2)],
            "detectionScore": round(float(box[4]), 4),
            "widthPx": round(x2 - x1, 1),
            "heightPx": round(y2 - y1, 1),
            "eyeDistancePx": round(eyes, 1),
            "sharpness": round(float(cv2.Laplacian(gray, cv2.CV_64F).var()), 1) if gray is not None else 0,
            "brightness": round(float(gray.mean()), 1) if gray is not None else 0,
            "eligible": not reasons,
            "rejectionReasons": reasons,
        }

    def embed(self, image, box, landmarks):
        face = Face(bbox=box[:4], kps=landmarks, det_score=box[4])
        vector = np.asarray(self.recognizer.get(image, face), dtype=np.float32).ravel()
        norm = float(np.linalg.norm(vector))
        if not np.isfinite(vector).all() or not np.isfinite(norm) or norm < 1e-8:
            raise ValueError("Invalid recognition embedding")
        return vector / norm

    def load_references(self, source, *, det_size=None):
        metadata = []
        for label, path, relative_name in reference_files(source):
            with Image.open(path) as photo:
                rgb = np.asarray(ImageOps.exif_transpose(photo).convert("RGB"))
            image = cv2.cvtColor(rgb, cv2.COLOR_RGB2BGR)
            input_size = (det_size, det_size) if det_size else None
            detections = self.detect(image, input_size=input_size)
            if len(detections) != 1:
                size = input_size or self.detector.input_size
                raise ValueError(f"{relative_name}: expected exactly one face; found {len(detections)} (detector {size})")
            box, landmarks = detections[0]
            quality = self.measure(image, box, landmarks)
            if not quality["eligible"]:
                raise ValueError(f"{relative_name}: unusable reference: {', '.join(quality['rejectionReasons'])}")
            vector = self.embed(image, box, landmarks)
            self.references.append((label, relative_name, vector))
            metadata.append({"label": label, "file": relative_name, "quality": quality})
        return metadata

    def compare(self, image, box, landmarks):
        vector = self.embed(image, box, landmarks)
        groups = {}
        for label, name, reference in self.references:
            if vector.shape != reference.shape:
                raise ValueError("Reference and video embedding dimensions differ")
            score = float(np.clip(np.dot(vector, reference), -1, 1))
            groups.setdefault(label, []).append({"file": name, "cosine": round(score, 6)})
        results = []
        for label, scores in groups.items():
            best = max(scores, key=lambda item: item["cosine"])
            results.append({
                "label": label,
                "maxCosine": best["cosine"],
                "meanCosine": round(sum(item["cosine"] for item in scores) / len(scores), 6),
                "bestReference": best["file"],
                "referenceScores": scores,
            })
        return sorted(results, key=lambda item: item["maxCosine"], reverse=True)
