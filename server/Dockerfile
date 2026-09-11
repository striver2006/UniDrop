# Multi-stage Docker build for minimal footprint
FROM golang:1.23-alpine AS builder

WORKDIR /build

COPY go.mod go.sum ./
RUN go mod download

COPY . .
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -ldflags="-w -s" -o unidrop-server ./cmd/unidrop-server

FROM alpine:3.20

RUN apk --no-cache add ca-certificates tzdata

WORKDIR /app
COPY --from=builder /build/unidrop-server /app/unidrop-server

EXPOSE 8080 3478/udp

ENTRYPOINT ["/app/unidrop-server"]
