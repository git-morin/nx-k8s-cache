FROM scratch
COPY target/x86_64-unknown-linux-musl/release/nx-cache-server /nx-cache-server
EXPOSE 8080
ENTRYPOINT ["/nx-cache-server"]
