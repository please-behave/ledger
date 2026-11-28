FROM rust:1.100.0-slim-trixie@sha256:24516ae4487ab804e3db145c93f7240e58e225587ab9b7c62ea31a218727bf67 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY migrations ./migrations
COPY src ./src
COPY why/contracts/ingest/*.schema.json ./why/contracts/ingest/
RUN cargo build --release --locked

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:29ee70762214fe8190cea04ef1c0a84a8365879e047ce8f911ae5a5ab2355fa2
COPY --from=build /src/target/release/ledger /usr/local/bin/ledger
USER nonroot
ENTRYPOINT ["/usr/local/bin/ledger"]
