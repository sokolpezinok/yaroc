FROM docker.io/python:3.11-slim-bookworm

RUN apt update && \
    apt install -y libcairo2-dev libgirepository1.0-dev

# TODO: this could speed everything up but it doesn't help for some reason
# apt install -y python3-serial-asyncio python3-psutil python3-paho-mqtt \
#                python3-gpiozero python3-pydbus python3-pyudev python3-aiohttp \

WORKDIR /app
COPY pyproject.toml .
COPY src src
RUN pip install .
