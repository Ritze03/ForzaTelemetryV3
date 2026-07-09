#!/usr/bin/env python3
"""Synthetic FH6 Data-Out sender for testing ForzaTelemetryV3 without the game.

Emits valid 324-byte little-endian packets (see docs/forza-fh6-packet-format.md).
Scenarios:
  circle  – drive a circle around a map centre (moving position + heading), for
            minimap / co-op testing. Use --phase to offset multiple cars.
  accel   – repeated 0->Vmax pulls with rpm sweeps + gear changes, for the
            dashboard / acceleration / power-curve tabs.
  idle    – stationary, engine at idle.

Examples:
  python sim.py --port 1337 --scenario accel
  python sim.py --port 1338 --scenario circle --phase 0   --car 1001
  python sim.py --port 1339 --scenario circle --phase 2.1 --car 2002
"""
import argparse
import math
import socket
import struct
import time

# Field order matches ForzaPacket::from_bytes exactly. '<' = little-endian, no padding.
FMT = "<" + (
    "iI"        # is_race_on, timestamp_ms
    "fff"       # engine max / idle / current rpm
    "fff"       # acceleration x y z
    "fff"       # velocity x y z
    "fff"       # angular velocity x y z
    "fff"       # yaw pitch roll
    "ffff"      # normalized suspension travel
    "ffff"      # tire slip ratio
    "ffff"      # wheel rotation speed
    "iiii"      # wheel on rumble strip
    "iiii"      # wheel in puddle
    "ffff"      # surface rumble
    "ffff"      # tire slip angle
    "ffff"      # tire combined slip
    "ffff"      # suspension travel meters
    "iiiii"     # car ordinal, class, pi, drivetrain, cylinders
    "I"         # car group
    "ff"        # smashable vel diff, mass
    "fff"       # position x y z
    "fff"       # speed power torque
    "ffff"      # tire temp
    "fff"       # boost fuel distance
    "ffff"      # best/last/current lap, current race time
    "H"         # lap number
    "B"         # race position
    "BBBBB"     # accel brake clutch handbrake gear
    "bbb"       # steer, driving line, ai brake diff
)
BASE_LEN = struct.calcsize(FMT)  # 323
PAD = b"\x00"                    # trailing byte the game includes -> 324

MAX_RPM = 7600.0
IDLE_RPM = 900.0


def clampi(v, lo, hi):
    return max(lo, min(hi, int(v)))


def gear_for_speed(kmh):
    for g, top in enumerate([40, 80, 120, 160, 210, 999], start=1):
        if kmh <= top:
            return g
    return 6


def rpm_for_speed(kmh, gear):
    # rev range within a gear: idle at the gear's bottom, near-max at its top
    band = [0, 40, 80, 120, 160, 210, 320][gear]
    prev = [0, 0, 40, 80, 120, 160, 210][gear]
    frac = 0.0 if band == prev else (kmh - prev) / (band - prev)
    return IDLE_RPM + (MAX_RPM - IDLE_RPM) * min(max(frac, 0.05), 1.0)


def build(is_race_on, ts, rpm, ax, ay, az, vx, vy, vz, yaw,
          px, py, pz, speed_ms, power, torque, boost, fuel, dist,
          car, gear, accel, brake, steer, lap, race_pos):
    vals = [
        1 if is_race_on else 0, ts & 0xFFFFFFFF,
        MAX_RPM, IDLE_RPM, rpm,
        ax, ay, az,
        vx, vy, vz,
        0.0, 0.0, 0.0,          # angular velocity
        yaw, 0.0, 0.0,          # yaw pitch roll
        0.5, 0.5, 0.5, 0.5,     # norm susp
        0.0, 0.0, 0.0, 0.0,     # slip ratio
        0.0, 0.0, 0.0, 0.0,     # wheel rot
        0, 0, 0, 0,             # rumble strip
        0, 0, 0, 0,             # puddle
        0.0, 0.0, 0.0, 0.0,     # surface rumble
        0.0, 0.0, 0.0, 0.0,     # slip angle
        0.0, 0.0, 0.0, 0.0,     # combined slip
        0.0, 0.0, 0.0, 0.0,     # susp meters
        car, 4, 800, 2, 6,      # ordinal, class=S1, pi, drivetrain=AWD, cyl
        1,                      # car group
        0.0, 0.0,               # smashable
        px, py, pz,
        speed_ms, power, torque,
        185.0, 185.0, 190.0, 190.0,   # tire temps (F)
        boost, fuel, dist,
        0.0, 0.0, 0.0, ts / 1000.0,   # laps
        lap, race_pos,
        accel, brake, 0, 0, gear,
        steer, 0, 0,
    ]
    return struct.pack(FMT, *vals) + PAD


def run(args):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    dst = (args.ip, args.port)
    t0 = time.time()
    dt = 1.0 / args.hz
    n = 0
    dist = 0.0
    fuel = 1.0
    prev_speed = 0.0
    prev_heading = None
    while True:
        t = time.time() - t0
        ts = int(t * 1000)

        if args.scenario == "idle":
            pkt = build(True, ts, IDLE_RPM, 0, 0, 0, 0, 0, 0, 0.0,
                        args.cx, 0.0, args.cz, 0.0, 0.0, 0.0, 0.0, fuel, dist,
                        args.car, 0, 0, 0, 0, 0, 0)

        elif args.scenario == "accel":
            # 14s cycle: 12s pull 0->~270 km/h, 2s coast/reset
            cyc = t % 14.0
            if cyc < 12.0:
                kmh = 270.0 * (cyc / 12.0) ** 0.85
                accel, brake = 255, 0
            else:
                kmh = max(0.0, 270.0 * (1 - (cyc - 12.0) / 2.0))
                accel, brake = 0, 200
            speed_ms = kmh / 3.6
            gear = gear_for_speed(kmh)
            rpm = rpm_for_speed(kmh, gear)
            # crude power curve peaking mid-range
            power = max(0.0, 220000.0 * math.sin(min(rpm / MAX_RPM, 1.0) * math.pi * 0.9))
            torque = power / max(rpm * 2 * math.pi / 60.0, 1.0)
            boost = max(0.0, 12.0 * (rpm / MAX_RPM))
            long_g = 9.81 * (1.2 if accel else -1.0)
            dist += speed_ms * dt
            fuel = max(0.0, fuel - 0.00002)
            pkt = build(True, ts, rpm, 0, 0, long_g, 0, 0, speed_ms, 0.0,
                        args.cx, 0.0, args.cz, speed_ms, power, torque, boost, fuel, dist,
                        args.car, gear, accel, brake, 0, 1, args.race_pos)

        elif args.scenario == "figure8":
            # Lemniscate of Gerono — rich lateral+longitudinal G for the traction circle.
            w = args.speed / args.radius
            a = w * t
            R = args.radius
            px = args.cx + R * math.cos(a)
            pz = args.cz + R * math.sin(a) * math.cos(a)
            vx = -R * w * math.sin(a)
            vz = R * w * math.cos(2 * a)
            speed_ms = math.hypot(vx, vz)
            heading = math.atan2(vx, vz)
            lat_a = long_a = 0.0
            if prev_heading is not None and dt > 0:
                long_a = (speed_ms - prev_speed) / dt
                dyaw = math.atan2(math.sin(heading - prev_heading),
                                  math.cos(heading - prev_heading)) / dt
                lat_a = speed_ms * dyaw
            prev_speed, prev_heading = speed_ms, heading
            kmh = speed_ms * 3.6
            gear = gear_for_speed(kmh)
            rpm = rpm_for_speed(kmh, gear)
            dist += speed_ms * dt
            steer = clampi(lat_a * 6.0, -127, 127)
            pkt = build(True, ts, rpm, lat_a, 0, long_a, vx, 0, vz, heading,
                        px, 0.0, pz, speed_ms, 120000.0, 250.0, 6.0, fuel, dist,
                        args.car, gear, 200, 0, steer, 1, args.race_pos)

        else:  # circle
            omega = args.speed / args.radius          # rad/s
            theta = omega * t + args.phase
            px = args.cx + args.radius * math.cos(theta)
            pz = args.cz + args.radius * math.sin(theta)
            heading = theta + math.pi / 2.0            # tangent
            vx = -args.speed * math.sin(theta)
            vz = args.speed * math.cos(theta)
            speed_ms = args.speed
            kmh = speed_ms * 3.6
            gear = gear_for_speed(kmh)
            rpm = rpm_for_speed(kmh, gear)
            lat_g = args.speed * omega / 9.81
            dist += speed_ms * dt
            fuel = max(0.0, fuel - 0.00001)
            # steer proportional to curvature direction (constant on a circle)
            steer = clampi(30, -127, 127)
            pkt = build(True, ts, rpm, lat_g * 9.81, 0, 0, vx, 0, vz, heading,
                        px, 0.0, pz, speed_ms, 120000.0, 250.0, 6.0, fuel, dist,
                        args.car, gear, 200, 0, steer, 1, args.race_pos)

        sock.sendto(pkt, dst)
        n += 1
        if args.count and n >= args.count:
            break
        time.sleep(dt)


def selftest():
    pkt = build(True, 0, 1000, 0, 0, 0, 0, 0, 0, 0.0, -8540.0, 0.0, 6738.0,
                0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1234, 1, 0, 0, 0, 0, 0)
    assert len(pkt) == 324, f"packet is {len(pkt)} bytes, expected 324"
    assert BASE_LEN == 323, f"base struct is {BASE_LEN}, expected 323"
    print("selftest ok: 324-byte packet, layout matches ForzaPacket")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--ip", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=1337)
    ap.add_argument("--hz", type=float, default=60.0)
    ap.add_argument("--scenario", choices=["circle", "figure8", "accel", "idle"], default="circle")
    ap.add_argument("--car", type=int, default=1001)
    ap.add_argument("--phase", type=float, default=0.0, help="circle: starting angle offset (rad)")
    ap.add_argument("--radius", type=float, default=300.0, help="circle radius (m)")
    ap.add_argument("--speed", type=float, default=40.0, help="circle speed (m/s)")
    ap.add_argument("--cx", type=float, default=-8540.0, help="centre world X")
    ap.add_argument("--cz", type=float, default=6738.0, help="centre world Z")
    ap.add_argument("--race-pos", type=int, default=1, dest="race_pos")
    ap.add_argument("--count", type=int, default=0, help="stop after N packets (0=forever)")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        selftest()
    else:
        try:
            run(args)
        except KeyboardInterrupt:
            pass
