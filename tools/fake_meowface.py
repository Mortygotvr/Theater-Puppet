"""A stand-in for MeowFace (or any phone speaking VTube Studio's protocol),
to test Theater Puppet's MeowFace input without a phone.

It listens where the phone does (UDP 21412). When Puppet asks
({"messageType":"iOSTrackingDataRequest", "sendForSeconds" or "time", "ports"}),
it sends face packets to the ports asked for, 30 a second, for as long as
asked: the mouth opening and closing (JawOpen), a blink every 2 s, the head
turning slowly.

    python tools/fake_meowface.py [seconds to run, default 30]

In Puppet: Configuration → MeowFace on, the phone's address 127.0.0.1;
General → Expressions and Head turn: Auto or MeowFace.
"""

import json
import math
import socket
import sys
import time

PHONE_PORT = 21412
run_for = float(sys.argv[1]) if len(sys.argv) > 1 else 30.0

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind(("0.0.0.0", PHONE_PORT))
sock.settimeout(1 / 30)
print(f"Fake MeowFace on UDP {PHONE_PORT} for {run_for:.0f} s: waiting for a request…", flush=True)

until = 0.0        # sending until then
targets = []       # (host, port) to send to
start = time.time()
sent = 0
while time.time() - start < run_for:
    try:
        data, (host, _) = sock.recvfrom(4096)
        ask = json.loads(data.decode("utf-8", "replace"))
        if ask.get("messageType") == "iOSTrackingDataRequest":
            # As MeowFace answers VSeeFace: sendForSeconds (else the documented "time")
            seconds = float(ask.get("sendForSeconds", ask.get("time", 0)) or 0)
            targets = [(host, int(p)) for p in ask.get("ports", [])]
            until = time.time() + seconds
            print(f"Request from {host} ({ask.get('sentBy')}): send to ports {[p for _, p in targets]} for {seconds:.0f} s", flush=True)
    except (socket.timeout, ValueError):
        pass
    if time.time() < until and targets:
        t = time.time() - start
        jaw = (math.sin(t * 3.0) + 1.0) / 2.0
        blink = 1.0 if (t % 2.0) < 0.15 else 0.0
        packet = {
            "Timestamp": int(time.time() * 1000),
            "Hotkey": -1,
            "FaceFound": True,
            "Rotation": {"x": 0.0, "y": math.sin(t * 0.8) * 20.0, "z": 0.0},
            "Position": {"x": 0.0, "y": 0.0, "z": 0.0},
            "EyeLeft": {"x": 0.0, "y": 0.0, "z": 0.0},
            "EyeRight": {"x": 0.0, "y": 0.0, "z": 0.0},
            "BlendShapes": [
                {"k": "JawOpen", "v": round(jaw, 3)},
                {"k": "EyeBlinkLeft", "v": blink},
                {"k": "EyeBlinkRight", "v": blink},
                {"k": "MouthSmileLeft", "v": 0.3},
                {"k": "MouthSmileRight", "v": 0.3},
            ],
        }
        raw = json.dumps(packet).encode()
        for target in targets:
            sock.sendto(raw, target)
        sent += 1
print(f"Done: {sent} packet(s) sent.", flush=True)
