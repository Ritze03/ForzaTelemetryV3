use std::io::{Cursor, Read};

/// FH6 telemetry packet — 323 meaningful bytes (game sends ~324, trailing byte ignored).
/// See docs/protocol/forza-fh6-packet-format.md for the full field reference.
#[derive(Debug, Clone, Default)]
pub struct ForzaPacket {
    pub is_race_on: i32,
    pub timestamp_ms: u32,

    pub engine_max_rpm: f32,
    pub engine_idle_rpm: f32,
    pub current_engine_rpm: f32,

    pub acceleration_x: f32,
    pub acceleration_y: f32,
    pub acceleration_z: f32,

    pub velocity_x: f32,
    pub velocity_y: f32,
    pub velocity_z: f32,

    pub angular_velocity_x: f32,
    pub angular_velocity_y: f32,
    pub angular_velocity_z: f32,

    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,

    pub normalized_suspension_travel_fl: f32,
    pub normalized_suspension_travel_fr: f32,
    pub normalized_suspension_travel_rl: f32,
    pub normalized_suspension_travel_rr: f32,

    pub tire_slip_ratio_fl: f32,
    pub tire_slip_ratio_fr: f32,
    pub tire_slip_ratio_rl: f32,
    pub tire_slip_ratio_rr: f32,

    pub wheel_rotation_speed_fl: f32,
    pub wheel_rotation_speed_fr: f32,
    pub wheel_rotation_speed_rl: f32,
    pub wheel_rotation_speed_rr: f32,

    pub wheel_on_rumble_strip_fl: i32,
    pub wheel_on_rumble_strip_fr: i32,
    pub wheel_on_rumble_strip_rl: i32,
    pub wheel_on_rumble_strip_rr: i32,

    pub wheel_in_puddle_fl: i32,
    pub wheel_in_puddle_fr: i32,
    pub wheel_in_puddle_rl: i32,
    pub wheel_in_puddle_rr: i32,

    pub surface_rumble_fl: f32,
    pub surface_rumble_fr: f32,
    pub surface_rumble_rl: f32,
    pub surface_rumble_rr: f32,

    pub tire_slip_angle_fl: f32,
    pub tire_slip_angle_fr: f32,
    pub tire_slip_angle_rl: f32,
    pub tire_slip_angle_rr: f32,

    pub tire_combined_slip_fl: f32,
    pub tire_combined_slip_fr: f32,
    pub tire_combined_slip_rl: f32,
    pub tire_combined_slip_rr: f32,

    pub suspension_travel_meters_fl: f32,
    pub suspension_travel_meters_fr: f32,
    pub suspension_travel_meters_rl: f32,
    pub suspension_travel_meters_rr: f32,

    pub car_ordinal: i32,
    pub car_class: i32,
    pub car_performance_index: i32,
    pub drivetrain_type: i32,
    pub num_cylinders: i32,

    // FH6-only fields (not in FM)
    pub car_group: u32,
    pub smashable_vel_diff: f32,
    pub smashable_mass: f32,

    pub position_x: f32,
    pub position_y: f32,
    pub position_z: f32,

    pub speed: f32,
    pub power: f32,
    pub torque: f32,

    pub tire_temp_fl: f32,
    pub tire_temp_fr: f32,
    pub tire_temp_rl: f32,
    pub tire_temp_rr: f32,

    pub boost: f32,
    pub fuel: f32,
    pub distance_traveled: f32,

    pub best_lap: f32,
    pub last_lap: f32,
    pub current_lap: f32,
    pub current_race_time: f32,

    pub lap_number: u16,
    pub race_position: u8,

    pub accel: u8,
    pub brake: u8,
    pub clutch: u8,
    pub hand_brake: u8,
    pub gear: u8,
    pub steer: i8,
    pub normalized_driving_line: i8,
    pub normalized_ai_brake_difference: i8,
}

impl ForzaPacket {
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 232 {
            // Minimum sanity check; real packets are ~323-324 bytes
            return None;
        }

        let mut c = Cursor::new(data);

        macro_rules! ri32 {
            () => {{
                let mut b = [0u8; 4];
                c.read_exact(&mut b).ok()?;
                i32::from_le_bytes(b)
            }};
        }
        macro_rules! ru32 {
            () => {{
                let mut b = [0u8; 4];
                c.read_exact(&mut b).ok()?;
                u32::from_le_bytes(b)
            }};
        }
        macro_rules! rf32 {
            () => {{
                let mut b = [0u8; 4];
                c.read_exact(&mut b).ok()?;
                f32::from_le_bytes(b)
            }};
        }
        macro_rules! ru16 {
            () => {{
                let mut b = [0u8; 2];
                c.read_exact(&mut b).ok()?;
                u16::from_le_bytes(b)
            }};
        }
        macro_rules! ru8 {
            () => {{
                let mut b = [0u8; 1];
                c.read_exact(&mut b).ok()?;
                b[0]
            }};
        }
        macro_rules! ri8 {
            () => {{
                let mut b = [0u8; 1];
                c.read_exact(&mut b).ok()?;
                b[0] as i8
            }};
        }

        Some(ForzaPacket {
            is_race_on: ri32!(),
            timestamp_ms: ru32!(),
            engine_max_rpm: rf32!(),
            engine_idle_rpm: rf32!(),
            current_engine_rpm: rf32!(),
            acceleration_x: rf32!(),
            acceleration_y: rf32!(),
            acceleration_z: rf32!(),
            velocity_x: rf32!(),
            velocity_y: rf32!(),
            velocity_z: rf32!(),
            angular_velocity_x: rf32!(),
            angular_velocity_y: rf32!(),
            angular_velocity_z: rf32!(),
            yaw: rf32!(),
            pitch: rf32!(),
            roll: rf32!(),
            normalized_suspension_travel_fl: rf32!(),
            normalized_suspension_travel_fr: rf32!(),
            normalized_suspension_travel_rl: rf32!(),
            normalized_suspension_travel_rr: rf32!(),
            tire_slip_ratio_fl: rf32!(),
            tire_slip_ratio_fr: rf32!(),
            tire_slip_ratio_rl: rf32!(),
            tire_slip_ratio_rr: rf32!(),
            wheel_rotation_speed_fl: rf32!(),
            wheel_rotation_speed_fr: rf32!(),
            wheel_rotation_speed_rl: rf32!(),
            wheel_rotation_speed_rr: rf32!(),
            wheel_on_rumble_strip_fl: ri32!(),
            wheel_on_rumble_strip_fr: ri32!(),
            wheel_on_rumble_strip_rl: ri32!(),
            wheel_on_rumble_strip_rr: ri32!(),
            wheel_in_puddle_fl: ri32!(),
            wheel_in_puddle_fr: ri32!(),
            wheel_in_puddle_rl: ri32!(),
            wheel_in_puddle_rr: ri32!(),
            surface_rumble_fl: rf32!(),
            surface_rumble_fr: rf32!(),
            surface_rumble_rl: rf32!(),
            surface_rumble_rr: rf32!(),
            tire_slip_angle_fl: rf32!(),
            tire_slip_angle_fr: rf32!(),
            tire_slip_angle_rl: rf32!(),
            tire_slip_angle_rr: rf32!(),
            tire_combined_slip_fl: rf32!(),
            tire_combined_slip_fr: rf32!(),
            tire_combined_slip_rl: rf32!(),
            tire_combined_slip_rr: rf32!(),
            suspension_travel_meters_fl: rf32!(),
            suspension_travel_meters_fr: rf32!(),
            suspension_travel_meters_rl: rf32!(),
            suspension_travel_meters_rr: rf32!(),
            car_ordinal: ri32!(),
            car_class: ri32!(),
            car_performance_index: ri32!(),
            drivetrain_type: ri32!(),
            num_cylinders: ri32!(),
            car_group: ru32!(),
            smashable_vel_diff: rf32!(),
            smashable_mass: rf32!(),
            position_x: rf32!(),
            position_y: rf32!(),
            position_z: rf32!(),
            speed: rf32!(),
            power: rf32!(),
            torque: rf32!(),
            tire_temp_fl: rf32!(),
            tire_temp_fr: rf32!(),
            tire_temp_rl: rf32!(),
            tire_temp_rr: rf32!(),
            boost: rf32!(),
            fuel: rf32!(),
            distance_traveled: rf32!(),
            best_lap: rf32!(),
            last_lap: rf32!(),
            current_lap: rf32!(),
            current_race_time: rf32!(),
            lap_number: ru16!(),
            race_position: ru8!(),
            accel: ru8!(),
            brake: ru8!(),
            clutch: ru8!(),
            hand_brake: ru8!(),
            gear: ru8!(),
            steer: ri8!(),
            normalized_driving_line: ri8!(),
            normalized_ai_brake_difference: ri8!(),
        })
    }

    /// Re-serialize to the 324-byte wire format (323 meaningful + 1 trailing pad).
    /// Inverse of `from_bytes`; used to relay locally-received telemetry over co-op.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(324);
        macro_rules! w { ($v:expr) => { b.extend_from_slice(&$v.to_le_bytes()); } }
        w!(self.is_race_on); w!(self.timestamp_ms);
        w!(self.engine_max_rpm); w!(self.engine_idle_rpm); w!(self.current_engine_rpm);
        w!(self.acceleration_x); w!(self.acceleration_y); w!(self.acceleration_z);
        w!(self.velocity_x); w!(self.velocity_y); w!(self.velocity_z);
        w!(self.angular_velocity_x); w!(self.angular_velocity_y); w!(self.angular_velocity_z);
        w!(self.yaw); w!(self.pitch); w!(self.roll);
        w!(self.normalized_suspension_travel_fl); w!(self.normalized_suspension_travel_fr);
        w!(self.normalized_suspension_travel_rl); w!(self.normalized_suspension_travel_rr);
        w!(self.tire_slip_ratio_fl); w!(self.tire_slip_ratio_fr);
        w!(self.tire_slip_ratio_rl); w!(self.tire_slip_ratio_rr);
        w!(self.wheel_rotation_speed_fl); w!(self.wheel_rotation_speed_fr);
        w!(self.wheel_rotation_speed_rl); w!(self.wheel_rotation_speed_rr);
        w!(self.wheel_on_rumble_strip_fl); w!(self.wheel_on_rumble_strip_fr);
        w!(self.wheel_on_rumble_strip_rl); w!(self.wheel_on_rumble_strip_rr);
        w!(self.wheel_in_puddle_fl); w!(self.wheel_in_puddle_fr);
        w!(self.wheel_in_puddle_rl); w!(self.wheel_in_puddle_rr);
        w!(self.surface_rumble_fl); w!(self.surface_rumble_fr);
        w!(self.surface_rumble_rl); w!(self.surface_rumble_rr);
        w!(self.tire_slip_angle_fl); w!(self.tire_slip_angle_fr);
        w!(self.tire_slip_angle_rl); w!(self.tire_slip_angle_rr);
        w!(self.tire_combined_slip_fl); w!(self.tire_combined_slip_fr);
        w!(self.tire_combined_slip_rl); w!(self.tire_combined_slip_rr);
        w!(self.suspension_travel_meters_fl); w!(self.suspension_travel_meters_fr);
        w!(self.suspension_travel_meters_rl); w!(self.suspension_travel_meters_rr);
        w!(self.car_ordinal); w!(self.car_class); w!(self.car_performance_index);
        w!(self.drivetrain_type); w!(self.num_cylinders);
        w!(self.car_group); w!(self.smashable_vel_diff); w!(self.smashable_mass);
        w!(self.position_x); w!(self.position_y); w!(self.position_z);
        w!(self.speed); w!(self.power); w!(self.torque);
        w!(self.tire_temp_fl); w!(self.tire_temp_fr); w!(self.tire_temp_rl); w!(self.tire_temp_rr);
        w!(self.boost); w!(self.fuel); w!(self.distance_traveled);
        w!(self.best_lap); w!(self.last_lap); w!(self.current_lap); w!(self.current_race_time);
        w!(self.lap_number); w!(self.race_position);
        w!(self.accel); w!(self.brake); w!(self.clutch); w!(self.hand_brake); w!(self.gear);
        w!(self.steer); w!(self.normalized_driving_line); w!(self.normalized_ai_brake_difference);
        b.push(0); // trailing pad byte the game includes
        b
    }

    pub fn speed_kmh(&self) -> f32 {
        self.speed * 3.6
    }

    /// A paused game reports the car at the world origin with zero orientation.
    /// Used to suppress co-op map rendering (trail + marker) for a paused player.
    pub fn is_paused(&self) -> bool {
        self.position_x == 0.0
            && self.position_y == 0.0
            && self.position_z == 0.0
            && self.yaw == 0.0
            && self.pitch == 0.0
            && self.roll == 0.0
    }

    pub fn speed_mph(&self) -> f32 {
        self.speed * 2.236_94
    }

    pub fn power_ps(&self) -> f32 {
        self.power / 735.499
    }

    pub fn torque_nm(&self) -> f32 {
        self.torque
    }

    pub fn car_class_str(&self) -> &'static str {
        car_class_name(self.car_class)
    }

    pub fn drivetrain_str(&self) -> &'static str {
        match self.drivetrain_type {
            0 => "FWD",
            1 => "RWD",
            2 => "AWD",
            _ => "?",
        }
    }

    pub fn tire_temp_celsius(temp_f: f32) -> f32 {
        (temp_f - 32.0) * 5.0 / 9.0
    }

}

/// Class letter for a raw `CarClass` int (0=D … 7=X).
pub fn car_class_name(class: i32) -> &'static str {
    match class {
        0 => "D",
        1 => "C",
        2 => "B",
        3 => "A",
        4 => "S1",
        5 => "S2",
        6 => "R",
        7 => "X",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_to_from_bytes() {
        // Fill a packet with distinct values so a mis-ordered field is caught.
        let mut p = ForzaPacket::default();
        p.is_race_on = 1;
        p.timestamp_ms = 123_456;
        p.current_engine_rpm = 6543.0;
        p.speed = 55.5;
        p.position_x = -12345.6;
        p.position_z = 9876.5;
        p.yaw = 1.23;
        p.car_ordinal = 4242;
        p.gear = 4;
        p.accel = 200;
        p.steer = -42;
        p.lap_number = 7;
        p.race_position = 3;
        let bytes = p.to_bytes();
        assert_eq!(bytes.len(), 324, "wire packet must be 324 bytes");
        let back = ForzaPacket::from_bytes(&bytes).expect("parse");
        assert_eq!(back.timestamp_ms, p.timestamp_ms);
        assert_eq!(back.current_engine_rpm, p.current_engine_rpm);
        assert_eq!(back.speed, p.speed);
        assert_eq!(back.position_x, p.position_x);
        assert_eq!(back.position_z, p.position_z);
        assert_eq!(back.car_ordinal, p.car_ordinal);
        assert_eq!(back.gear, p.gear);
        assert_eq!(back.steer, p.steer);
        assert_eq!(back.race_position, p.race_position);
    }

    #[test]
    fn rejects_undersized_packets() {
        // A too-short buffer must be rejected, never partially parsed.
        assert!(ForzaPacket::from_bytes(&[]).is_none());
        assert!(ForzaPacket::from_bytes(&[0u8; 100]).is_none());
        assert!(ForzaPacket::from_bytes(&[0u8; 231]).is_none());
        // A full-length packet parses.
        assert!(ForzaPacket::from_bytes(&ForzaPacket::default().to_bytes()).is_some());
    }
}
