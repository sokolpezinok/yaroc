FROM docker.io/python:3.11-slim-bullseye
RUN apt update && apt install -y build-essential ninja-build patchelf

RUN apt install -y libdbus-1-dev libglib2.0-dev

COPY . .
RUN pip install .
