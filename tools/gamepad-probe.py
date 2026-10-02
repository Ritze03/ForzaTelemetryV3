#!/usr/bin/env python3
"""Gamepad probe (research tool for docs/features/gamepad.md).

Lists every evdev gamepad (has BTN_SOUTH + ABS_RX/RY) with vendor:product, driver-ish info
(name, phys, uniq) and axis ranges, then prints live right-stick / trigger / button events.
Read-only, never grabs a device. Needs the `input` group, like the app.

    python3 tools/gamepad-probe.py          # list + live events from all pads
    python3 tools/gamepad-probe.py --list   # list only

Why it exists: the Steam Input virtual pad (28de:11ff) and the physical pad can both be
present and both deliver events; this shows which is which. Requires python-evdev.
"""
import selectors
import sys

from evdev import InputDevice, ecodes as e, list_devices

STEAM_VIRTUAL = (0x28DE, 0x11FF)


def is_gamepad(d):
    caps = d.capabilities()
    keys = caps.get(e.EV_KEY, [])
    absc = [a[0] if isinstance(a, tuple) else a for a in caps.get(e.EV_ABS, [])]
    return e.BTN_SOUTH in keys and e.ABS_RX in absc and e.ABS_RY in absc


def main():
    pads = []
    for path in list_devices():
        try:
            d = InputDevice(path)
        except PermissionError:
            print(f"{path}: permission denied (add yourself to the 'input' group)")
            continue
        if not is_gamepad(d):
            continue
        pads.append(d)
        tag = "STEAM VIRTUAL (skipped by the app)" if (d.info.vendor, d.info.product) == STEAM_VIRTUAL else "physical?"
        print(f"{d.path}: {d.name!r} {d.info.vendor:04x}:{d.info.product:04x} bus={d.info.bustype} phys={d.phys!r} uniq={d.uniq!r}  [{tag}]")
        for code, info in d.capabilities().get(e.EV_ABS, []):
            if code in (e.ABS_X, e.ABS_Y, e.ABS_RX, e.ABS_RY, e.ABS_Z, e.ABS_RZ, e.ABS_HAT0X, e.ABS_HAT0Y):
                print(f"    {e.ABS[code]}: min={info.min} max={info.max} flat={info.flat}")
    if "--list" in sys.argv or not pads:
        return
    print("\nLive events (Ctrl+C to stop):")
    sel = selectors.DefaultSelector()
    for d in pads:
        sel.register(d, selectors.EVENT_READ)
    watch = {e.ABS_RX, e.ABS_RY, e.ABS_Z, e.ABS_RZ, e.ABS_HAT0X, e.ABS_HAT0Y}
    try:
        while True:
            for key, _ in sel.select():
                d = key.fileobj
                for ev in d.read():
                    if ev.type == e.EV_ABS and ev.code in watch:
                        print(f"{d.path} {d.name[:24]:24} {e.ABS[ev.code]:10} {ev.value}")
                    elif ev.type == e.EV_KEY:
                        name = e.BTN.get(ev.code) or e.KEY.get(ev.code) or ev.code
                        print(f"{d.path} {d.name[:24]:24} {name} {'down' if ev.value else 'up'}")
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
