#!/usr/bin/env python

from mininet.topo import Topo
from mininet.net import Mininet
from mininet.node import Node
from mininet.log import setLogLevel, info
from mininet.cli import CLI

from threading import Thread
from time import sleep
import argparse

class LinuxRouter( Node ):
    "A Node with IP forwarding enabled."

    def config( self, **params ):
        super( LinuxRouter, self).config( **params )
        self.cmd( 'sysctl net.ipv4.ip_forward=1' )

    def terminate( self ):
        self.cmd( 'sysctl net.ipv4.ip_forward=0' )
        super( LinuxRouter, self ).terminate()

class NetworkTopo( Topo ):
    "A LinuxRouter connecting three IP subnets"

    def build( self, **_opts ):

        defaultIP = '10.0.0.2/24'  
        router = self.addHost( 'r0', cls=LinuxRouter, ip=defaultIP )

        s1, s2, s3, s4, s5 = [ self.addSwitch( s ) for s in ( 's1', 's2', 's3', 's4', 's5') ]

        self.addLink( s4, router, intfName2='r0-eth0',
                      params2={ 'ip' : defaultIP })  
        self.addLink( s5, router, intfName2='r0-eth1',
                      params2={ 'ip' : '10.0.1.2/24' })
        self.addLink( s3, router, intfName2='r0-eth2',
                      params2={ 'ip' : '10.1.0.2/24' } )

        self.addLink( s1, s4)
        self.addLink( s2, s5)

        h1 = self.addHost( 'h1', ip='10.0.0.1/24',
                           defaultRoute='via 10.0.0.2' )
    
        h3 = self.addHost( 'h3', ip='10.1.0.1/24',
                           defaultRoute='via 10.1.0.2' )

        self.addLink( h1, s1, params1={'ip' : '10.0.0.1/24'}, intfName1='h1-eth0')
        self.addLink( h1, s2, params1={'ip' : '10.0.1.1/24'}, intfName1='h1-eth1')
        
        for h, s in [(h3, s3) ]:
            self.addLink( h, s )


def shutdown_link(router, switch, cut):
    sleep(16)
    if cut:
        switch.cmd('ip link set dev s4-eth1 down')
        switch.cmd('ip link set dev s4-eth2 down')  
    else:
        switch.cmd('tc qdisc add dev s4-eth1 root netem delay 500ms')
        switch.cmd('tc qdisc add dev s4-eth2 root netem delay 500ms')
    router.cmd('echo "************* Shutting down link *********************" >> {}'.format(args.file))
    info('*** Shutting down link\n')
    sleep(5)
    if cut:
        switch.cmd('ip link set dev s4-eth1 up')
        switch.cmd('ip link set dev s4-eth2 up')
    else:
        switch.cmd('tc qdisc del dev s4-eth1 root netem delay 500ms')
        switch.cmd('tc qdisc del dev s4-eth2 root netem delay 500ms')
    router.cmd('echo "************* Link is up ! *********************" >> {}'.format(args.file))
    info('*** Link is up\n')

def run(args):
    "Test linux router"
    topo = NetworkTopo()
    net = Mininet( topo=topo)  
    net.start()

    #configure source routing
    client = net['h1']
    client.cmd('ip rule add from 10.0.1.1 lookup 100')
    client.cmd('ip route add default via 10.0.1.2 dev h1-eth1 table 100')

    s1 = net['s1']
    s1.cmd('tc qdisc add dev s1-eth1 root netem delay {}ms rate {}Mbps loss random {}%'.format(args.delay, args.bandwidth, args.loss))
    s1.cmd('tc qdisc add dev s1-eth2 root netem delay {}ms rate {}Mbps loss random {}%'.format(args.delay, args.bandwidth, args.loss))

    s2 = net['s2']
    s1.cmd('tc qdisc add dev s2-eth1 root netem delay {}ms rate {}Mbps loss random {}%'.format(args.delaybackup, args.bandwidthbackup, args.lossbackup))
    s1.cmd('tc qdisc add dev s2-eth2 root netem delay {}ms rate {}Mbps loss random {}%'.format(args.delaybackup, args.bandwidthbackup, args.lossbackup))
    server = net['h3']

    info('Starting server\n')
    server.cmd('tshark -i h3-eth0 -w capture_server0_quic.pcapng &')
    if args.scheduler == "last":
        server.cmd('cd .. ; ./server -s 10.1.0.1:8080 -p fileexchange --multipath --rcvd-threshold 10000000 --max-bidi-remote 7000 --scheduler last --timeout 30000 --duplicate > ./experiment6/server_out &')
    elif args.scheduler == "lrtt":
        #server.cmd('cd .. ; ./server -s 10.1.0.1:8080 -p fileexchange --multipath --rcvd-threshold 50  --max-bidi-remote 7000 --scheduler lrtt2 --timeout 30000 --ack-eliciting-timer 25 --duplicate > ./experiment6/server_out &')
        server.cmd('cd .. ; ./server -s 10.1.0.1:8080 -p fileexchange --multipath --rcvd-threshold 500000  --max-bidi-remote 7000 --scheduler lrtt --timeout 30000 --ack-eliciting-timer 250000 --duplicate > ./experiment6/server_out &')
    elif args.scheduler == "normal":
        server.cmd('cd .. ; ./server -s 10.1.0.1:8080 -p fileexchange --multipath --rcvd-threshold 10000000 --max-bidi-remote 7000 --scheduler normal --timeout 30000 --duplicate > ./experiment6/server_out &')
    else:
        info('Unknown scheduler type: {}\n'.format(args.scheduler))
        return

    print("cutting link : " + str(args.cut))
    router = net['r0']
    info('Starting shutdown thread\n')
    s4 = net['s4']
    t = Thread(target=shutdown_link, args=(router,s4,args.cut,))
    t.start()
    
    sleep(8)
    info('Starting client\n')
    if args.scheduler == "last":
            client.cmd('cd .. ; ./client -s 10.1.0.1:8080 -p fileexchange --multipath -A 10.0.0.1:5050 -A 10.0.1.1:3355 --file file -n 80 --rcvd-threshold 10000000 --time 200 --ack-eliciting-timer 100000000  --scheduler last --timeout 30000 --duplicate > {}'.format(args.file))
    elif args.scheduler == "lrtt":
        client.cmd('cd .. ; ./client -s 10.1.0.1:8080 -p fileexchange --multipath -A 10.0.0.1:5050 -A 10.0.1.1:3355 --file file -n 80 --rcvd-threshold 250000 --time 200 --ack-eliciting-timer 250000  --scheduler lrtt --timeout 30000 --duplicate > {}'.format(args.file))
        #client.cmd('cd .. ; ./client -s 10.1.0.1:8080 -p fileexchange --multipath -A 10.0.0.1:5050 -A 10.0.1.1:3355 --file file -n 80 --rcvd-threshold 25 --time 200 --ack-eliciting-timer 25  --scheduler lrtt2 --timeout 30000 --duplicate > {}'.format(args.file))
    elif args.scheduler == "normal":
        client.cmd('cd .. ; ./client -s 10.1.0.1:8080 -p fileexchange --multipath -A 10.0.0.1:5050 -A 10.0.1.1:3355 --file file -n 80 --rcvd-threshold 10000000 --time 200 --ack-eliciting-timer 100000000  --scheduler normal --timeout 30000 --duplicate > {}'.format(args.file))
    sleep(1)

    info( '*** Routing Table on Router:\n' )
    info( net[ 'r0' ].cmd( 'route' ) )
    #CLI( net )
    t.join()
    net.stop()

if __name__ == '__main__':
    parser = argparse.ArgumentParser()

    parser.add_argument('-f', '--file', type=str, help='Client output file', required=False, default='./experiment6/outputQuic')
    parser.add_argument('-d', '--delay', type=int, help='Delay to apply on the primary link [ms]', required=False, default=10)
    parser.add_argument('-l', '--loss', type=float, help='Loss to apply on the primary link [%]', required=False, default=0.0)
    parser.add_argument('-b', '--bandwidth', type=int, help='Bandwidth to shape the primary link [Mbps]', required=False, default=100)
    parser.add_argument('-db', '--delaybackup', type=int, help='Delay to apply on the backup link [ms]', required=False, default=50)
    parser.add_argument('-lb', '--lossbackup', type=float, help='Loss to apply on the backup link [%]', required=False, default=0.0)
    parser.add_argument('-bb', '--bandwidthbackup', type=int, help='Bandwidth to shape the backup link [Mbps]', required=False, default=100)
    parser.add_argument('-c', '--cut', type=bool, help='Whether to cut the link or not', required=False, default=False)
    parser.add_argument('-s', '--scheduler', type=str, help='Type of scheduler to use [last - lrtt - normal]', required=False, default="last")
    args = parser.parse_args()
    
    setLogLevel( 'info' )
    run(args)