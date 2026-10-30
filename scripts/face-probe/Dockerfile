FROM python:3.12-slim-bookworm@sha256:392307d22300de8b5986851a12d9176dfc0fc073e65bf6523ebd7dcbeb23564e

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    ORT_DISABLE_TELEMETRY=1 \
    FACE_PROBE_MODELS=/models \
    FACE_PROBE_REFERENCES=/references \
    FACE_PROBE_DATA=/data

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates ffmpeg libgl1 libglib2.0-0 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 probe \
    && useradd --system --uid 10001 --gid 10001 --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin probe \
    && install --directory --mode=0700 --owner=10001 --group=10001 /data

WORKDIR /app
COPY requirements.txt ./
RUN python -m pip install --no-cache-dir -r requirements.txt

# Runtime only: no PyTorch/export tooling, downloaded models, reference photos or reports in the image.
COPY assets.py engine.py reference_inputs.py probe.py watch.py watch_store.py watch_stream.py ./
COPY dashboard.py dashboard_worker.py dashboard_reports.py ./
COPY web/ ./web/

USER 10001:10001
EXPOSE 8765
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=3 \
    CMD python -c "import urllib.request; urllib.request.urlopen('http://127.0.0.1:8765/health', timeout=2).close()"
ENTRYPOINT ["python", "dashboard.py", "--host", "0.0.0.0"]
