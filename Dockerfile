FROM ghcr.io/rust-cross/manylinux_2_28-cross:armv7

COPY conf/sources.list /etc/apt/sources.list
RUN apt update
RUN dpkg --add-architecture armhf

RUN apt install -y gcc-arm-linux-gnueabihf pkg-config python3 python3-pip python3-venv protobuf-compiler
RUN apt install -y libudev-dev:armhf libdbus-1-dev:armhf

RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup default stable && rustup target add armv7-unknown-linux-gnueabihf

WORKDIR /app
COPY . .

ENV PKG_CONFIG_ALLOW_CROSS=1
ENV PKG_CONFIG_PATH=/usr/lib/arm-linux-gnueabihf/pkgconfig
ENV CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=arm-linux-gnueabihf-gcc
ENV PYO3_CROSS_PYTHON_VERSION=3.12
RUN cargo build --release --target armv7-unknown-linux-gnueabihf
