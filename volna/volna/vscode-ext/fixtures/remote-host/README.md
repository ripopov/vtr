# Disposable remote workspace host

This image supplies a Linux workspace host for testing the native query server
through VS Code Remote SSH or Dev Containers. The VS Code webview remains on
the client machine. It accepts public-key authentication for `tester` only;
there is no password login. Publish its SSH port on loopback when testing locally.

From the repository root:

```sh
docker build -t vtr-volna-remote-test volna/volna/vscode-ext/fixtures/remote-host
docker run -d --name vtr-volna-remote-test -p 127.0.0.1::22 vtr-volna-remote-test
docker port vtr-volna-remote-test 22
```

Generate an ephemeral client key outside the repository. Copy only its public
key into `/home/tester/.ssh/authorized_keys` in the container, then set owner
`tester:tester` and mode `600`. Each container generates its own host keys; verify
the host key before connecting. Use an isolated SSH configuration and VS Code
user-data directory so testing does not alter ordinary development sessions.

Copy a checkout, including the initialized `ext/elkrs` dependency, into
`/workspace/vtr` and give `tester` ownership. Include any uncommitted query source
under test. Build the host-specific child as `tester`:

```sh
docker exec -u tester -w /workspace/vtr vtr-volna-remote-test \
  node volna/volna/vscode-ext/build-server.cjs
```

The resulting binary belongs in `vscode-ext/bin/linux-arm64` or `linux-x64`,
according to the container architecture. Supply the WASM `media` assets from
`volna/volna/web/build.sh`; they are independent of the server host. Install or
launch the extension on the remote workspace host, open a trace inside that
workspace, and follow the relay smoke checks in `volna/volna/VERIFICATION.md`.
Inspect child processes inside the container, not on the client machine.

After verification, remove this named container and the ephemeral client key.
The image contains no client key or repository data and can be reused. An image
build alone is not evidence that the remote extension flow works.
