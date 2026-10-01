# nnInteractive served over the FERRUM Engine Protocol (ferrum-engine/1).
# Build from bridges/: see nninteractive/docker-compose.yml and docs/ai-demo.md.
# Needs an NVIDIA GPU with the NVIDIA Container Toolkit. Model weights are
# downloaded on first start into the /models volume (CC BY-NC-SA 4.0,
# research use only; never part of the image).
FROM pytorch/pytorch:2.8.0-cuda12.8-cudnn9-runtime

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    NNINTERACTIVE_MODEL_DIR=/models

WORKDIR /app
COPY pyproject.toml ./
COPY ferrum_bridges ./ferrum_bridges
RUN pip install --no-cache-dir ".[nninteractive]"

VOLUME /models
EXPOSE 8765
# 0.0.0.0 inside the container only; compose publishes the port on 127.0.0.1.
ENTRYPOINT ["ferrum-bridge", "--host", "0.0.0.0", "--port", "8765", "nninteractive"]
