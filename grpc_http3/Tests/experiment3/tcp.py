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

        s1, s2, s3 = [ self.addSwitch( s ) for s in ( 's1', 's2', 's3' ) ]

        self.addLink( s1, router, intfName2='r0-eth0',
                      params2={ 'ip' : defaultIP } )  
        self.addLink( s2, router, intfName2='r0-eth1',
                      params2={ 'ip' : '10.0.1.2/24' } )
        self.addLink( s3, router, intfName2='r0-eth2',
                      params2={ 'ip' : '10.1.0.2/24' } )

        h1 = self.addHost( 'h1', ip='10.0.0.1/24',
                           defaultRoute='via 10.0.0.2' )
    
        h3 = self.addHost( 'h3', ip='10.1.0.1/24',
                           defaultRoute='via 10.1.0.2' )

        self.addLink( h1, s1, params1={'ip' : '10.0.0.1/24'}, intfName1='h1-eth0')
        self.addLink( h1, s2, params1={'ip' : '10.0.1.1/24'}, intfName1='h1-eth1')
        
        for h, s in [(h3, s3) ]:
            self.addLink( h, s )

def shutdown_link(router, switch):
    sleep(16)
    switch.cmd('ip link set dev s1-eth1 down')
    switch.cmd('ip link set dev s1-eth2 down')  
    router.cmd('echo "************* Shutting down link *********************" >> {}'.format(args.file))
    info('*** Shutting down link\n')
    sleep(5)
    switch.cmd('ip link set dev s1-eth1 up')
    switch.cmd('ip link set dev s1-eth2 up') 
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
    server.cmd('cd .. ; ./HTTP2/serverHTTP2 -s 10.1.0.1:8080 -s 10.1.0.1:8081 -p fileexchange &')

    router = net['r0']
    info('Starting shutdown thread\n')
    t = Thread(target=shutdown_link, args=(router ,s1, ))
    t.start()

    sleep(8)
    info('Starting client\n')
    client.cmd('cd .. ; ./HTTP2/clientHTTP2 -s 10.1.0.1:8080 -s 10.1.0.1:8081 -p fileexchange -A 10.0.0.1 -A 10.0.1.1 --file file -n 80 --rtime 500 --time 200 --ptimer 0 > {}'.format(args.file))
    sleep(1)
    
    info( '*** Routing Table on Router:\n' )
    info( net[ 'r0' ].cmd( 'route' ) )
    #CLI( net )
    t.join()
    net.stop()


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    
    parser.add_argument('-f', '--file', type=str, help='Client output file', required=False, default='./experiment3/outputHTTP')
    parser.add_argument('-d', '--delay', type=int, help='Delay to apply on the primary link [ms]', required=False, default=10)
    parser.add_argument('-l', '--loss', type=float, help='Loss to apply on the primary link [%]', required=False, default=0.0)
    parser.add_argument('-b', '--bandwidth', type=int, help='Bandwidth to shape the primary link [Mbps]', required=False, default=100)
    parser.add_argument('-db', '--delaybackup', type=int, help='Delay to apply on the backup link [ms]', required=False, default=50)
    parser.add_argument('-lb', '--lossbackup', type=float, help='Loss to apply on the backup link [%]', required=False, default=0.0)
    parser.add_argument('-bb', '--bandwidthbackup', type=int, help='Bandwidth to shape the backup link [Mbps]', required=False, default=100)
    args = parser.parse_args()
    
    
    setLogLevel( 'info' )
    run(args)
