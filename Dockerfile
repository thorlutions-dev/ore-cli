FROM rust:1.75 as builder

ENV PATH="/root/.local/share/solana/install/active_release/bin:${PATH}"

ADD . /opt/
WORKDIR /opt/

# RUN sh -c "$(curl -sSfL https://release.solana.com/stable/install)" && cargo build --release
RUN cargo build --release

FROM debian:bookworm-slim as runner

COPY --from=builder /opt/target/release/ore /usr/local/bin/ore
COPY --from=builder /opt/entrypoint.sh /usr/local/bin/entrypoint.sh

ENTRYPOINT [ "sh", "/usr/local/bin/entrypoint.sh" ]
