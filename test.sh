#!/bin/bash

set -e

RUST_BACKTRACE=1 cargo run --example server &
sleep 1
curl localhost:8080
