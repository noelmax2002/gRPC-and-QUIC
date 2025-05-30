#!/usr/bin/env bash

trap 'kill -9 $(jobs -p %1)' 2

sleep 10 &

wait
