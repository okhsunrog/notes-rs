# notes-rs build and deployment helpers

server_target := "x86_64-unknown-linux-musl"
server_release_dir := "release-server"
server_archive := "notes-server-release.tar.gz"

# Build the server as a portable static Linux binary for the VPS.
# sqlite-vec 0.1.9 uses BSD u_int*_t aliases in its bundled C source; map
# only those aliases to their standard stdint.h names for the musl target.
build-server:
    CC_x86_64_unknown_linux_musl=musl-gcc \
        CFLAGS_x86_64_unknown_linux_musl="-Du_int8_t=uint8_t -Du_int16_t=uint16_t -Du_int64_t=uint64_t" \
        cargo build --release --locked --target {{ server_target }} -p notes-server

# Create the artifact consumed by cloud-forge's notes_server role.
package-server: build-server
    #!/usr/bin/env bash
    set -euo pipefail

    rm -rf {{ server_release_dir }}
    mkdir -p {{ server_release_dir }}/bin
    cp target/{{ server_target }}/release/notes-server {{ server_release_dir }}/bin/
    cp server/config.example.toml {{ server_release_dir }}/
    tar -czf {{ server_archive }} -C {{ server_release_dir }} .
    echo "Created {{ server_archive }}"
    tar -tzf {{ server_archive }}

# Build, package and deploy the server and its complete public edge configuration.
deploy-server: package-server
    cd ../../cloud-forge && ansible-playbook site-moscow.yml --tags notes-server,haproxy,nginx,certbot,network

clean-server:
    rm -rf {{ server_release_dir }} {{ server_archive }}
