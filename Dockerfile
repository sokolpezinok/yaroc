FROM docker.io/python:3.11-slim-bookworm

RUN apt update && apt install -y cargo libudev-dev pkg-config

ENV VIRTUAL_ENV=/opt/venv
RUN python3 -m venv $VIRTUAL_ENV
ENV PATH="$VIRTUAL_ENV/bin:$PATH"

WORKDIR /app
COPY . .
RUN pip install maturin && maturin develop
