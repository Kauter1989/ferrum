# TotalSegmentator served over the FERRUM Engine Protocol (ferrum-engine/1).
# Build from bridges/: see totalsegmentator/docker-compose.yml and docs/ai-demo.md.
# GPU recommended (CPU works with --fast, slowly). Weights are downloaded on
# first use into the /weights volume; usage statistics are switched off.
FROM pytorch/pytorch:2.8.0-cuda12.8-cudnn9-runtime

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    TOTALSEG_HOME_DIR=/weights/home \
    TOTALSEG_WEIGHTS_PATH=/weights/nnunet

WORKDIR /app
COPY pyproject.toml ./
COPY ferrum_bridges ./ferrum_bridges
RUN pip install --no-cache-dir ".[totalsegmentator]"

VOLUME /weights
EXPOSE 8765
ENTRYPOINT ["ferrum-bridge", "--host", "0.0.0.0", "--port", "8765", "totalsegmentator"]
