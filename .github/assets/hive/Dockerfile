FROM ubuntu

COPY dist/reth /usr/local/bin

COPY LICENSE-* ./

EXPOSE 30303 30303/udp 9001 8545 8546
ENTRYPOINT ["/usr/local/bin/reth"]