# syntax=docker/dockerfile:1.7
# Linux image for the network lab: tc-spike (pattern sharer + headless viewer) plus the
# tools to shape traffic (tc netem, iptables). Built from the repo root:
#   docker compose -f bench/netlab/compose.yml build

FROM rust:1.95-bookworm AS build
# The prebuilt libwebrtc ships a hermetic libc++ that needs clang 21 or later (bookworm has
# 14), so take clang from LLVM's own apt repository. The X11/GL headers are what the
# prebuilt libwebrtc links against.
RUN curl -fsSL https://apt.llvm.org/llvm-snapshot.gpg.key -o /etc/apt/trusted.gpg.d/apt.llvm.org.asc \
    && echo "deb http://apt.llvm.org/bookworm/ llvm-toolchain-bookworm-21 main" \
        > /etc/apt/sources.list.d/llvm.list \
    && apt-get update && apt-get install -y --no-install-recommends \
        clang-21 libclang-21-dev lld-21 pkg-config \
        libx11-dev libxext-dev libxfixes-dev libxdamage-dev libxcomposite-dev libxrandr-dev \
        libxtst-dev libgl1-mesa-dev libglib2.0-dev libdrm-dev libgbm-dev libva-dev \
    && rm -rf /var/lib/apt/lists/*
ENV CC=clang-21 CXX=clang++-21 LIBCLANG_PATH=/usr/lib/llvm-21/lib
# libwebrtc's Linux screen capture (xdg portal) calls GLib/GIO, but webrtc-sys only uses their
# headers and leaves linking to the app. Link them here; the real Linux build (later
# milestone) will need the same.
ENV RUSTFLAGS="-C link-arg=-lgio-2.0 -C link-arg=-lgobject-2.0 -C link-arg=-lglib-2.0"
WORKDIR /src
COPY . .
# Cache the registry and the target dir (with the ~280 MB libwebrtc download) across builds.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p twincursor-media-spike && cp target/release/tc-spike /tc-spike

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
        iproute2 iptables procps ca-certificates \
        libx11-6 libxext6 libxfixes3 libxdamage1 libxcomposite1 libxrandr2 libxtst6 \
        libgl1 libglib2.0-0 libdrm2 libgbm1 libva2 libva-drm2 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /tc-spike /usr/local/bin/tc-spike
COPY bench/netlab/shape.sh /usr/local/bin/shape
ENTRYPOINT ["sleep", "infinity"]
