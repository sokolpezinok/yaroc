# Optimally build with buildah using local sccache, it's much faster.
# The following platforms have been tested:
# * linux/amd64
# * linux/arm64
# * linux/arm/v7
#
# Run ideally with buildah:
# buildah bud -t yaroc --layers --platform linux/arm64 -v ~/.cache/sccache:/root/.cache/sccache .

FROM rust:1.82-slim AS chef
RUN apt update && apt install -y python3-pip python3-venv sccache protobuf-compiler
ENV RUSTC_WRAPPER=sccache
RUN cargo install cargo-chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

RUN python3 -m venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH"
RUN pip install maturin[patchelf]

COPY . .
RUN cd python && maturin build --release
