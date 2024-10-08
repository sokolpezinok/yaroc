# Optimally build with buildah using local sccache, it's much faster.
# The option `--platform linux/arm64` has been tested, others will come.
# buildah bud -t yaroc --layers --platform linux/arm64 -v /home/lukas/.cache/sccache:/root/.cache/sccache .

FROM rust:1.81-slim

RUN apt update
RUN apt install -y gcc pkg-config python3 python3-pip python3-venv protobuf-compiler libudev-dev libdbus-1-dev sccache

RUN python3 -m venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH"
RUN pip install maturin[patchelf]

WORKDIR /app
COPY . .

ENV RUSTC_WRAPPER=sccache
RUN maturin build --release
