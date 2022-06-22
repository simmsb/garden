#!/bin/bash

set -e

cargo objcopy --release -- -O binary target/build.bin
bossac -i -d --port=${1:-ttyACM0} -o 0x2000 -U -e -w -v target/build.bin -R
