#!/usr/bin/env bash

target/release/httplatencies \
  --urls \
      http://127.0.0.6:8080/noop \
      http://127.0.0.2:8080/noop \
      http://127.0.0.3:8080/noop \
      http://127.0.0.4:8080/noop \
      http://127.0.0.5:8080/noop \
  --local-ips \
      127.0.100.6 \
      127.0.100.2 \
      127.0.100.3 \
      127.0.100.4 \
      127.0.100.5 \
  --task-count $1 \
  --probe-count $2 \
  --clients $3
