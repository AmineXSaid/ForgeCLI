port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
nohup python3 -m http.server "$port" --bind 127.0.0.1 --directory site > server.log 2>&1 &
echo $! > server.pid
echo "$port" > port.txt
sleep 1
