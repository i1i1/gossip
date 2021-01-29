# Gossip

Simple distributed application which was made only using tokio.
Instances can broadcast information with each other.

## Run
Launch 3 instances:

Instance which is not connected to anyone which will send random data every 10 seconds.
```sh
$ ca r -- '0.0.0.0:10000' -t 10
```

Instance which is connected to first instance which will send random data every 9 seconds.
```sh
$ ca r -- '0.0.0.0:10001' -c '0.0.0.0:10000' -t 9
```

Instance which is connected to second instance which will send random data every 8 seconds.
```sh
$ ca r -- '0.0.0.0:10002' -c '0.0.0.0:10001' -t 8
```
