# MONAI Label bridge served over the FERRUM Engine Protocol (ferrum-engine/1).
# Build from bridges/: see compose.yml. The bridge needs no GPU: it is a
# client of a running MONAI Label server, which runs the models.
FROM python:3.11-slim

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

WORKDIR /app
COPY pyproject.toml ./
COPY ferrum_bridges ./ferrum_bridges
RUN pip install --no-cache-dir ".[monailabel]"

EXPOSE 8765
# 0.0.0.0 inside the container only; compose publishes the port on 127.0.0.1.
ENTRYPOINT ["ferrum-bridge", "--host", "0.0.0.0", "--port", "8765", "monailabel"]
