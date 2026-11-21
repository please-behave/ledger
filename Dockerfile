FROM rust:1.100.0-slim-trixie@sha256:6f14b2b4b278ec0d50aa1d69541c304ce0f95e0f6268487aa9924af1c2e9aebe AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY src ./src
RUN cargo build --release --locked

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cb87acc574abe719b6e5c547384defa2f7b1f24af9605d3f618cf852495d8be2
COPY --from=build /src/target/release/ledger /usr/local/bin/ledger
USER nonroot
ENTRYPOINT ["/usr/local/bin/ledger"]
