#!/bin/bash
set -euo pipefail

# Configuration
IMAGE_TYPE=${IMAGE_TYPE:-lite} # "lite" or "desktop"
DEB_ARCH=${DEB_ARCH:-armhf}    # "armhf" or "arm64"
MOUNT_POINT=${MOUNT_POINT:-rpi_image}
DATE=${DATE:-2025-12-04}
PI_PASSWORD=${PI_PASSWORD:-yaroc}
REPO_URL=${REPO_URL:-https://github.com/sokolpezinok/yaroc}
YAROC_REF=${YAROC_REF:-master}
OUTPUT_IMAGE=${OUTPUT_IMAGE:-custom-${DEB_ARCH}.img}
COMPRESS=${COMPRESS:-0}        # Set to 1 to compress output image with xz
YAROC_WHEEL=${YAROC_WHEEL:-}   # Optional path to .whl file
YAROC_EXTRAS=${YAROC_EXTRAS:-display} # Optional comma-separated extras (empty for none)

LOOP_DEV=""
COPIED_QEMU=""

cleanup() {
    echo "Cleaning up..."
    if [ -n "$COPIED_QEMU" ] && [ -e "$COPIED_QEMU" ]; then
        sudo rm -f "$COPIED_QEMU" 2>/dev/null || true
        COPIED_QEMU=""
    fi
    sudo umount -l "$MOUNT_POINT/dev/shm" 2>/dev/null || true
    sudo umount -l "$MOUNT_POINT/dev/pts" 2>/dev/null || true
    sudo umount -l "$MOUNT_POINT/proc" 2>/dev/null || true
    sudo umount -l "$MOUNT_POINT/sys" 2>/dev/null || true
    sudo umount -l "$MOUNT_POINT/dev" 2>/dev/null || true
    sudo umount -l "$MOUNT_POINT/boot/firmware" 2>/dev/null || true
    sudo umount -l "$MOUNT_POINT" 2>/dev/null || true
    if [ -n "$LOOP_DEV" ]; then
        sudo losetup -d "$LOOP_DEV" 2>/dev/null || true
        LOOP_DEV=""
    fi
}
trap cleanup EXIT

find_qemu_bin() {
    local candidates=()
    if [ "$DEB_ARCH" = "arm64" ]; then
        candidates=(/usr/bin/qemu-aarch64-static /usr/bin/qemu-aarch64)
    elif [ "$DEB_ARCH" = "armhf" ]; then
        candidates=(/usr/bin/qemu-arm-static /usr/bin/qemu-arm /usr/bin/qemu-armhf)
    else
        echo "Unsupported DEB_ARCH: $DEB_ARCH (expected armhf or arm64)" >&2
        exit 1
    fi

    for bin in "${candidates[@]}"; do
        if [ -x "$bin" ]; then
            echo "$bin"
            return 0
        fi
    done

    echo "Could not find QEMU user-mode binary (checked: ${candidates[*]})." >&2
    echo "Install qemu-user and qemu-user-binfmt (Debian/Ubuntu) or qemu-user (Arch)." >&2
    exit 1
}

QEMU_BIN="$(find_qemu_bin)"
echo "Using QEMU binary: $QEMU_BIN"

if [ "$IMAGE_TYPE" = "lite" ]; then
    IMAGE_FILE="${DATE}-raspios-trixie-${DEB_ARCH}-lite.img.xz"
    URL="https://downloads.raspberrypi.com/raspios_lite_${DEB_ARCH}/images/raspios_lite_${DEB_ARCH}-${DATE}/${IMAGE_FILE}"
elif [ "$IMAGE_TYPE" = "desktop" ]; then
    IMAGE_FILE="${DATE}-raspios-trixie-${DEB_ARCH}.img.xz"
    URL="https://downloads.raspberrypi.com/raspios_${DEB_ARCH}/images/raspios_${DEB_ARCH}-${DATE}/${IMAGE_FILE}"
else
    echo "Unsupported IMAGE_TYPE: $IMAGE_TYPE (expected lite or desktop)" >&2
    exit 1
fi

# Download if missing
if [ ! -f "$IMAGE_FILE" ]; then
    echo "Downloading $IMAGE_FILE..."
    wget --progress=dot:giga -O "${IMAGE_FILE}.tmp" "$URL"
    mv "${IMAGE_FILE}.tmp" "$IMAGE_FILE"
else
    echo "$IMAGE_FILE already exists, skipping download."
fi

# Extract and resize image
if [ ! -f "$OUTPUT_IMAGE" ]; then
    echo "Extracting $IMAGE_FILE to $OUTPUT_IMAGE..."
    xz -d -c "$IMAGE_FILE" > "${OUTPUT_IMAGE}.tmp"
    mv "${OUTPUT_IMAGE}.tmp" "$OUTPUT_IMAGE"
    echo "Expanding $OUTPUT_IMAGE by 2048 MiB..."
    dd if=/dev/zero bs=1M count=2048 >> "$OUTPUT_IMAGE"
else
    echo "$OUTPUT_IMAGE already exists, skipping extraction and expansion."
fi

LOOP_DEV=$(sudo losetup -fP --show "$OUTPUT_IMAGE")
echo "Using loop device: $LOOP_DEV"

sudo parted -s "$LOOP_DEV" resizepart 2 100%
sudo partprobe "$LOOP_DEV" 2>/dev/null || true
sudo e2fsck -fy "${LOOP_DEV}p2" || { rc=$?; [ "$rc" -le 1 ]; }
sudo resize2fs "${LOOP_DEV}p2"

# Mount the .img
mkdir -p "$MOUNT_POINT"
sudo mount "${LOOP_DEV}p2" "$MOUNT_POINT"
sudo mkdir -p "$MOUNT_POINT/boot/firmware"
sudo mount "${LOOP_DEV}p1" "$MOUNT_POINT/boot/firmware"

sudo cp "$QEMU_BIN" "${MOUNT_POINT}${QEMU_BIN}"
COPIED_QEMU="${MOUNT_POINT}${QEMU_BIN}"

# Mount virtual filesystems
pushd "$MOUNT_POINT" >/dev/null
sudo mount --bind /dev dev
sudo mount --bind /sys sys
sudo mount --bind /proc proc
sudo mount --bind /dev/pts dev/pts
sudo mount --bind /dev/shm dev/shm
popd >/dev/null

# Automated setup inside chroot
# Fix sudo hostname warning
HOST_NAME="$(hostname)"
if ! sudo grep -qF "127.0.0.1 $HOST_NAME" "$MOUNT_POINT/etc/hosts"; then
    echo "127.0.0.1 $HOST_NAME" | sudo tee -a "$MOUNT_POINT/etc/hosts" >/dev/null
fi

sudo chroot "$MOUNT_POINT" env -i \
  PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  HOME=/root \
  DEBIAN_FRONTEND=noninteractive \
  bash -c '
  set -e
  echo "Acquire::Retries \"5\"; Acquire::http::Timeout \"15\";" > /etc/apt/apt.conf.d/99-ci-retries
  sed -i "s|http://raspbian.raspberrypi.com/raspbian|http://archive.raspbian.org/raspbian|g" /etc/apt/sources.list /etc/apt/sources.list.d/* 2>/dev/null || true
  apt-get update
  apt-get install -y --no-install-recommends locales-all vim git build-essential python3-dev curl gpg
  touch /boot/firmware/ssh
  echo "pi:$1" | chpasswd
' _ "$PI_PASSWORD"

sudo cp "$(dirname "$0")/setup_yaroc.sh" "$MOUNT_POINT/tmp/setup_yaroc.sh"

WHEEL_NAME=""
if [ -n "$YAROC_WHEEL" ]; then
    if [ ! -f "$YAROC_WHEEL" ]; then
        echo "YAROC_WHEEL file not found: $YAROC_WHEEL" >&2
        exit 1
    fi

    WHEEL_NAME="$(basename "$YAROC_WHEEL")"
    echo "Copying local wheel $WHEEL_NAME into chroot..."
    sudo cp "$YAROC_WHEEL" "$MOUNT_POINT/tmp/$WHEEL_NAME"
    YAROC_BASE="/tmp/$WHEEL_NAME"
else
    YAROC_BASE="yaroc"
fi

if [ -n "$YAROC_EXTRAS" ]; then
    YAROC_SPEC="${YAROC_BASE}[${YAROC_EXTRAS}]"
else
    YAROC_SPEC="$YAROC_BASE"
fi

sudo chmod +x "$MOUNT_POINT/tmp/setup_yaroc.sh"
sudo chroot "$MOUNT_POINT" env -i \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME=/root \
    DEBIAN_FRONTEND=noninteractive \
    YAROC_REF="$YAROC_REF" \
    /tmp/setup_yaroc.sh "$REPO_URL" "$YAROC_SPEC"

sudo sed -i "s|http://archive.raspbian.org/raspbian|http://raspbian.raspberrypi.com/raspbian|g" "$MOUNT_POINT"/etc/apt/sources.list "$MOUNT_POINT"/etc/apt/sources.list.d/* 2>/dev/null || true
sudo rm -f "$MOUNT_POINT/etc/apt/apt.conf.d/99-ci-retries" "$MOUNT_POINT/tmp/setup_yaroc.sh"
if [ -n "$WHEEL_NAME" ]; then
    sudo rm -f "$MOUNT_POINT/tmp/$WHEEL_NAME"
fi

echo "Cleaning caches and build dependencies inside chroot..."
sudo chroot "$MOUNT_POINT" env DEBIAN_FRONTEND=noninteractive bash -c '
  set -e
  apt-get purge -y build-essential python3-dev
  apt-get autoremove -y --purge
  apt-get clean
  rm -rf /var/lib/apt/lists/* /root/.cache /home/pi/.cache/uv /tmp/* /var/tmp/*
'

echo "Zeroing free space..."
sudo dd if=/dev/zero of="$MOUNT_POINT/zero.fill" bs=1M status=none 2>/dev/null || true
sync
sudo rm -f "$MOUNT_POINT/zero.fill"
sync

# Unmount before optional compression
cleanup
trap - EXIT

if [ "$COMPRESS" = "1" ] || [ "$COMPRESS" = "true" ]; then
    echo "Compressing $OUTPUT_IMAGE..."
    xz -T0 -z -f "$OUTPUT_IMAGE"
    echo "Image creation complete: ${OUTPUT_IMAGE}.xz"
else
    echo "Image creation complete: $OUTPUT_IMAGE"
fi
