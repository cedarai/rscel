FROM ubuntu:latest

RUN apt update && apt upgrade -y
RUN apt install -y curl build-essential

RUN cd /tmp && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs > rustup.sh && sh rustup.sh --default-host aarch64-unknown-linux-gnu --default-toolchain nightly --profile default -y


