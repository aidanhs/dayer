set -o errexit
set -o pipefail
set -o nounset

RUSTFLAGS='--cfg nonnative_tls' cargo build --release --target x86_64-unknown-linux-musl
