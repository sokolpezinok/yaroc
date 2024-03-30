#!/bin/sh

cp /home/runner/work/yaroc/yaroc/conf/sources.list /etc/apt/sources.list
apt update
dpkg --add-architecture armhf

apt install -y gcc-arm-linux-gnueabihf pkg-config python3 python3-pip python3-venv protobuf-compiler
apt install -y libudev-dev:armhf
