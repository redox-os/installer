#!/usr/bin/env bash

IMAGE=test.bin

QEMU_ARGS=(
	-cpu max
	-machine q35
	-m 2048
	-smp 4
	-serial mon:stdio
	-netdev user,id=net0
	-device e1000,netdev=net0
)

if [ -e /dev/kvm ]
then
	QEMU_ARGS+=(-accel kvm)
fi

set -ex

cargo build --release

rm -f "${IMAGE}"
fallocate -l 1GiB "${IMAGE}"

target/release/redox_installer -c test.toml "${IMAGE}"

qemu-system-x86_64 "${QEMU_ARGS[@]}" -drive "file=${IMAGE},format=raw"
