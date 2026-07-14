# Forza Horizon 6 — Data Out Packet Format

Source: https://support.forza.net/hc/en-us/articles/51744149102611-Forza-Horizon-6-Data-Out-Documentation

Total packet size: **324 bytes**, little-endian.

## Type notation
- `S<n>` — signed integer, n bits
- `U<n>` — unsigned integer, n bits
- `F<n>` — floating point, n bits

## Struct layout

```
S32 IsRaceOn                          // 1 = race active, 0 = menus/stopped

U32 TimestampMS                       // can overflow to 0

// Engine
F32 EngineMaxRpm
F32 EngineIdleRpm
F32 CurrentEngineRpm

// Acceleration in car-local space (X=right, Y=up, Z=forward)
F32 AccelerationX
F32 AccelerationY
F32 AccelerationZ

// Velocity in car-local space (X=right, Y=up, Z=forward)
F32 VelocityX
F32 VelocityY
F32 VelocityZ

// Angular velocity in car-local space, rad/s (X=pitch, Y=yaw, Z=roll)
F32 AngularVelocityX
F32 AngularVelocityY
F32 AngularVelocityZ

// Orientation, radians
F32 Yaw
F32 Pitch
F32 Roll

// Suspension travel normalized: 0.0 = max stretch, 1.0 = max compression
F32 NormalizedSuspensionTravelFrontLeft
F32 NormalizedSuspensionTravelFrontRight
F32 NormalizedSuspensionTravelRearLeft
F32 NormalizedSuspensionTravelRearRight

// Tire slip ratio: 0 = 100% grip, |ratio| > 1.0 = loss of grip
F32 TireSlipRatioFrontLeft
F32 TireSlipRatioFrontRight
F32 TireSlipRatioRearLeft
F32 TireSlipRatioRearRight

// Wheel rotation speed, rad/s
F32 WheelRotationSpeedFrontLeft
F32 WheelRotationSpeedFrontRight
F32 WheelRotationSpeedRearLeft
F32 WheelRotationSpeedRearRight

// 1 = wheel on rumble strip, 0 = off
S32 WheelOnRumbleStripFrontLeft
S32 WheelOnRumbleStripFrontRight
S32 WheelOnRumbleStripRearLeft
S32 WheelOnRumbleStripRearRight

// 1 = wheel in puddle, 0 = not
S32 WheelInPuddleFrontLeft
S32 WheelInPuddleFrontRight
S32 WheelInPuddleRearLeft
S32 WheelInPuddleRearRight

// Surface rumble (non-dimensional, for force feedback scaling)
F32 SurfaceRumbleFrontLeft
F32 SurfaceRumbleFrontRight
F32 SurfaceRumbleRearLeft
F32 SurfaceRumbleRearRight

// Tire slip angle: 0 = 100% grip, |angle| > 1.0 = loss of grip
F32 TireSlipAngleFrontLeft
F32 TireSlipAngleFrontRight
F32 TireSlipAngleRearLeft
F32 TireSlipAngleRearRight

// Tire combined slip: 0 = 100% grip, |slip| > 1.0 = loss of grip
F32 TireCombinedSlipFrontLeft
F32 TireCombinedSlipFrontRight
F32 TireCombinedSlipRearLeft
F32 TireCombinedSlipRearRight

// Actual suspension travel, meters
F32 SuspensionTravelMetersFrontLeft
F32 SuspensionTravelMetersFrontRight
F32 SuspensionTravelMetersRearLeft
F32 SuspensionTravelMetersRearRight

// Car identity
S32 CarOrdinal             // unique car make/model ID
S32 CarClass               // 0=D, 1=C, 2=B, 3=A, 4=S1, 5=S2, 6=R, 7=X
S32 CarPerformanceIndex    // 100 (worst) to 999 (best)
S32 DrivetrainType         // 0=FWD, 1=RWD, 2=AWD
S32 NumCylinders

// FH6-only fields (not present in Forza Motorsport)
U32 CarGroup
F32 SmashableVelDiff       // velocity loss from smashable object collision (m/s)
F32 SmashableMass          // mass of recently hit smashable object (kg)

// World position, meters
F32 PositionX
F32 PositionY
F32 PositionZ

// Dynamics
F32 Speed                  // m/s
F32 Power                  // watts
F32 Torque                 // newton-meters

// Tire temperature
F32 TireTempFrontLeft
F32 TireTempFrontRight
F32 TireTempRearLeft
F32 TireTempRearRight

F32 Boost                  // turbo/supercharger boost, PSI above atmospheric
F32 Fuel                   // 0.0 = empty, 1.0 = full
F32 DistanceTraveled       // total meters

// Lap times, seconds (0.0 if not applicable)
F32 BestLap
F32 LastLap
F32 CurrentLap
F32 CurrentRaceTime        // seconds since driving started

U16 LapNumber
U8  RacePosition

// Player inputs (0–255)
U8  Accel
U8  Brake
U8  Clutch
U8  HandBrake

U8  Gear
S8  Steer                  // -127 = full left, 0 = center, 127 = full right
S8  NormalizedDrivingLine  // -127 to 127
S8  NormalizedAIBrakeDifference  // -127 to 127
```

## Notes

- Data is sent **only while actively driving** — not during menus, pauses, replays, rewinds, or post-race.
- Transmission is **one-way UDP** (game sends, never receives).
- Packet format is **fixed** — no format selection like FM.
- **Avoid ports 5200–5300**: game binds its outgoing socket in this range.
- FH6 adds `CarGroup`, `SmashableVelDiff`, `SmashableMass` after `NumCylinders`. FM does not have these.
- FH6 does **not** include `TireWear` or `TrackOrdinal` (present in FM "Dash" format).
- FH6 supports localhost (127.0.0.1) natively — no loopback workaround needed.
- Configure in-game: **SETTINGS > HUD AND GAMEPLAY > Data Out**.
