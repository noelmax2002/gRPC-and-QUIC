#!/bin/sh

N=1
TRANSPORT=$1
DELAY=10
BW=100
LOSS=$2
PATHs=/home/noelmax/Bureau/Mémoire/gRPC-and-QUIC/grpc_http3
PROTO="fileexchange"
FILE="/home/noelmax/Bureau/Mémoire/gRPC-and-QUIC/grpc_http3/swift_file_examples/file"
OUTPUT_DIR="./${TRANSPORT}_test"

mkdir -p ${OUTPUT_DIR}

for i in $(seq 1 $N);
do
    echo "Running experiment $i"
    echo "Launching server"
    if [ "${TRANSPORT}" = "tcp" ]; then
	    ${PATHs}/target/release/serverHTTP2 -s 127.0.0.1:3131 -p ${PROTO} --server-pem ../server.pem --server-key ../server.key > /dev/null &
    else 
	    ${PATHs}/target/release/server -p ${PROTO} --max-bidi-remote 70000 --cert-file ${PATHs}/src/cert.crt --key-file ${PATHs}/src/cert.key > /dev/null &
    fi

    sleep 2
    echo "Apply link emulation"
    sudo tc qdisc add dev lo root netem delay ${DELAY}ms rate ${BW}Mbps loss random ${LOSS}%
    echo "Launching client"

    if [ "${TRANSPORT}" = "tcp" ]; then
        ${PATHs}/target/release/clientHTTP2 -p ${PROTO} --file ${FILE} -n 100 --time 50 --ptimer 0 --rtime 100000 -s 127.0.0.1:3131 --ca-cert ../ca.pem > ${OUTPUT_DIR}/output${i}
    else 
        ${PATHs}/target/release/client -p ${PROTO} --file ${FILE} -n 100 --time 50 --ack-eliciting-timer 10000000000 > ${OUTPUT_DIR}/output${i}
    fi

    sudo tc qdisc del dev lo root netem

    echo "Killing server"
    if [ "${TRANSPORT}" = "tcp" ]; then
        pkill -n serverHTTP2
    else 
        pkill -n server
    fi
done
