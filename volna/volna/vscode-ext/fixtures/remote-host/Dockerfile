# Disposable Linux workspace host for real VS Code Remote SSH/Dev Containers
# verification. Build the server here; reuse the platform-independent WASM assets.
FROM ubuntu:24.04
ARG RUST_TOOLCHAIN=nightly-2026-04-14
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl git nodejs openssh-server pkg-config \
    protobuf-compiler libssl-dev zlib1g-dev \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell /bin/bash tester \
    && passwd -d tester \
    && mkdir -p /run/sshd /workspace /home/tester/.ssh \
    && chown tester:tester /workspace /home/tester/.ssh \
    && chmod 700 /home/tester/.ssh \
    && printf '%s\n' 'PasswordAuthentication no' 'KbdInteractiveAuthentication no' \
       'PermitEmptyPasswords no' 'PermitRootLogin no' 'AllowUsers tester' \
       > /etc/ssh/sshd_config.d/volna-test.conf \
    && rm -f /etc/ssh/ssh_host_* \
    && printf '%s\n' 'export PATH=/home/tester/.cargo/bin:$PATH' > /etc/profile.d/volna-rust.sh
USER tester
ENV PATH=/home/tester/.cargo/bin:${PATH}
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --no-modify-path --profile minimal --default-toolchain "${RUST_TOOLCHAIN}" \
    && rustup component add rustfmt clippy \
    && rustup target add wasm32-unknown-unknown
WORKDIR /workspace
USER root
EXPOSE 22
CMD ["/bin/sh", "-c", "ssh-keygen -A && exec /usr/sbin/sshd -D -e"]
