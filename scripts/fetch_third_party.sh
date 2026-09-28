#!/bin/sh
# Fetches the third-party sources that the verification flows use, at the
# revisions the recorded results were produced with. They go to third_party/,
# which is not part of the repository.
set -e
cd "$(dirname "$0")/.."
mkdir -p third_party

fetch() {  # directory, URL, revision
    if [ ! -d "third_party/$1/.git" ]; then
        git clone -q "$2" "third_party/$1"
    fi
    git -C "third_party/$1" fetch -q origin
    git -C "third_party/$1" checkout -q "$3"
}

# ISA conformance tests (rv32ui, rv32mi) and their macro headers.
fetch riscv-tests https://github.com/riscv-software-src/riscv-tests.git bcffa2b3188b040c611f90dc0b6e422f54775a09
git -C third_party/riscv-tests submodule update -q --init env

# Formal ISA checks (scripts/formal.py).
fetch riscv-formal https://github.com/YosysHQ/riscv-formal.git c992aa61fdfe0846c5ed90324c596202a1c69b76

# The ULX3S reference constraints that fpga/ulx3s/ulx3s.lpf was taken from.
mkdir -p third_party/ulx3s
curl -fsSL -o third_party/ulx3s/ulx3s_v20.lpf \
    https://raw.githubusercontent.com/emard/ulx3s/master/doc/constraints/ulx3s_v20.lpf

echo "third_party/ ready"
