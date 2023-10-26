FROM docker.io/python:3.11-slim-bookworm

RUN apt update && \
    apt install -y python3-serial-asyncio python3-psutil python3-paho-mqtt \
                   python3-gpiozero python3-pydbus python3-pyudev python3-aiohttp

WORKDIR /app
COPY pyproject.toml .
COPY src src
RUN pip install .
