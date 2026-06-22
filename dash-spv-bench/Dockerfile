FROM debian:bookworm-slim

ARG DASHVERSION=23.1.4

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl iproute2 \
    && rm -rf /var/lib/apt/lists/*

# Download the Linux dashd matching the host CPU arch (amd64 -> x86_64, arm64 -> aarch64),
# keeping the same release used by contrib/setup-dashd.py.
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      amd64) dlarch=x86_64 ;; \
      arm64) dlarch=aarch64 ;; \
      *) echo "unsupported arch $arch" >&2; exit 1 ;; \
    esac; \
    url="https://github.com/dashpay/dash/releases/download/v${DASHVERSION}/dashcore-${DASHVERSION}-${dlarch}-linux-gnu.tar.gz"; \
    curl -fsSL "$url" -o /tmp/dashcore.tgz; \
    tar -xzf /tmp/dashcore.tgz -C /tmp; \
    cp "/tmp/dashcore-${DASHVERSION}/bin/dashd" /usr/local/bin/dashd; \
    rm -rf /tmp/dashcore*; \
    dashd --version | head -1
