#!/bin/bash

set -e

cargo run --example server &
sleep 1
curl localhost:8080
