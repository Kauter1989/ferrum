# nnInteractive served over the FERRUM Engine Protocol (ferrum-engine/1).
# Needs an NVIDIA GPU with the NVIDIA Container Toolkit; see docs/ai-demo.md.
# Model weights are downloaded on first start into the /models volume (they
# are not part of the image: CC BY-NC-SA 4.0, research use only).
FROM pytorch/pytorch:2.8.0-cuda12.8-cudnn9-runtime

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    NNINTERACTIVE_MODEL_DIR=/models

WORKDIR /app
COPY pyproject.toml ./
COPY ferrum_nninteractive ./ferrum_nninteractive
RUN pip install --no-cache-dir ".[nninteractive]"

VOLUME /models
EXPOSE 8765
# 0.0.0.0 inside the container only; docker-compose publishes the port on
# 127.0.0.1 of the host.
ENTRYPOINT ["ferrum-nninteractive", "--host", "0.0.0.0", "--port", "8765"]
