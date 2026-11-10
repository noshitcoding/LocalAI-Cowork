# Official source release, pinned because upstream no longer distributes images.
FROM golang:1.26.5-alpine AS build
RUN apk add --no-cache git
RUN CGO_ENABLED=0 go install github.com/minio/minio@9e49d5e7a648f00e26f2246f4dc28e6b07f8c84a
FROM alpine:3.23
RUN apk add --no-cache ca-certificates curl
COPY --from=build /go/bin/minio /usr/local/bin/minio
COPY deploy/enterprise/storage-entrypoint.sh /usr/local/bin/storage-entrypoint
ENTRYPOINT ["sh", "/usr/local/bin/storage-entrypoint"]
