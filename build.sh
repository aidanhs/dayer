set -o errexit
set -o pipefail
set -o nounset
# Ubuntu 16.04

#cd zlib
#    CC=musl-gcc ./configure --prefix=$(pwd)/dist --static
#    make -j4
#    make install
#    cd ..
cd openssl
    #CC=musl-gcc ./config --prefix=$(pwd)/dist --with-zlib-include=$(pwd)/../zlib/dist/include threads no-shared zlib no-asm no-hw no-dso
    CC=musl-gcc ./config --prefix=$(pwd)/dist threads no-shared no-zlib no-asm no-hw no-dso no-async no-afalgeng
    make depend
    make -j4
    make install
    cd ..
# OPENSSL_STATIC=1 - not compiled with fpic
# --target x86_64-unknown-linux-musl - libssl is compiled with glibc
OPENSSL_STATIC=1 OPENSSL_DIR=$(pwd)/openssl/dist cargo build --release --target x86_64-unknown-linux-musl
