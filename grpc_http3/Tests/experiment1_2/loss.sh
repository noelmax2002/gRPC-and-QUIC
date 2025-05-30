

for i in 0.0 0.5 1.0 1.5 2.0;
do
    echo "Running tcp experiment with loss $i"
    sh run.sh tcp $i
    echo "Running quic experiment with loss $i"
    sh run.sh quic $i 
done
