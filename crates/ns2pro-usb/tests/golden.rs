use ns2pro_proto::{Buttons, Stick};
use ns2pro_usb::{usbip::*, *};
use serde_json::Value;
fn hex(s: &Value) -> Vec<u8> {
    hex::decode(s.as_str().unwrap_or("")).unwrap()
}
#[test]
fn control_fixtures() {
    let rows: Value =
        serde_json::from_str(include_str!("../../../fixtures/viiper/control.json")).unwrap();
    let mut device = Ns2ProDevice::default();
    for row in rows.as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let setup: [u8; 8] = hex(&row["setup"]).try_into().unwrap();
        let result = device.control(setup, &hex(&row["out"]), &InputSample::default());
        if [
            "string_7",
            "hid_report_desc_iface1",
            "msos_unknown",
            "hid_get_report_feature",
            "vendor_unknown",
        ]
        .contains(&name)
        {
            assert!(result.is_err(), "intentional STALL: {name}");
        } else {
            assert_eq!(result.unwrap().0, hex(&row["response"]), "{name}");
        }
    }
}
fn state(s: &Value) -> ns2pro_proto::InputState {
    let n = |key: &str| s[key].as_i64().unwrap();
    ns2pro_proto::InputState {
        buttons: Buttons(n("Buttons") as u32),
        left_stick: Stick {
            x: n("LX") as u16,
            y: n("LY") as u16,
        },
        right_stick: Stick {
            x: n("RX") as u16,
            y: n("RY") as u16,
        },
        accel: [n("AccelX") as i16, n("AccelY") as i16, n("AccelZ") as i16],
        gyro: [n("GyroX") as i16, n("GyroY") as i16, n("GyroZ") as i16],
        battery_level: n("BatteryLevel") as u8,
        charging: s["Charging"].as_bool().unwrap(),
        external_power: s["ExternalPower"].as_bool().unwrap(),
    }
}
#[test]
fn all_scenario_fixtures() {
    let rows: Value =
        serde_json::from_str(include_str!("../../../fixtures/viiper/scenarios.json")).unwrap();
    for scenario in rows.as_array().unwrap() {
        let mut d = Ns2ProDevice::default();
        let mut sample = InputSample::default();
        let mut timestamp = 0;
        let mut imu = false;
        for step in scenario["steps"].as_array().unwrap() {
            let mut output = None;
            let result = match step["op"].as_str().unwrap() {
                "state" => {
                    sample.state = state(&step["state"]);
                    None
                }
                "out" => {
                    let data = hex(&step["data"]);
                    output = if step["ep"] == 1 {
                        d.interrupt_out(&data)
                    } else {
                        if data.first() == Some(&0x0c) && data.len() > 8 {
                            match data[3] {
                                4 => imu |= data[8] & 4 != 0,
                                5 => imu &= data[8] & 4 == 0,
                                3 => imu = false,
                                _ => {}
                            }
                        }
                        d.bulk_out(&data)
                    };
                    None
                }
                "in" => Some(if step["ep"] == 1 {
                    if imu {
                        timestamp += 4000
                    }
                    sample.motion_timestamp_us = timestamp;
                    d.interrupt_in(&sample).to_vec()
                } else {
                    d.pop_bulk_in().unwrap_or_default()
                }),
                "control" => {
                    let data = hex(&step["data"]);
                    if data[0..4] == [0x21, 9, 1, 2] {
                        assert_eq!(
                            d.control(data[..8].try_into().unwrap(), &data[8..], &sample),
                            Err(-32)
                        );
                        continue;
                    }
                    let (p, o) = d
                        .control(data[..8].try_into().unwrap(), &data[8..], &sample)
                        .unwrap();
                    output = o;
                    Some(p)
                }
                _ => panic!(),
            };
            if let Some(result) = result {
                let mut expected = hex(&step["response"]);
                if scenario["name"] == "flash_commands"
                    && step["op"] == "in"
                    && expected.get(12..16) == Some(&0x13000u32.to_le_bytes())
                {
                    // Intentional identity difference: retain the VIIPER fixture as provenance.
                    assert_eq!(&expected[18..34], b"VIIPER-NS2PRO-00");
                    let mut fallback = [0; 64];
                    fallback[2..18].copy_from_slice(b"NS2PRO-BRIDGE-00");
                    expected[16..].copy_from_slice(&fallback);
                }
                assert_eq!(result, expected, "{} / {}", scenario["name"], step);
            }
            if let Some(expected) = step["outputs"].as_array() {
                let expected = &expected[0];
                match output.unwrap() {
                    Output::PlayerLeds(v) => {
                        assert_eq!(u64::from(v), expected["leds"].as_u64().unwrap())
                    }
                    Output::Rumble { left, right } => {
                        assert_eq!(left.as_slice(), hex(&expected["left"]));
                        assert_eq!(right.as_slice(), hex(&expected["right"]));
                    }
                }
            } else {
                assert!(output.is_none(), "unexpected output {step}")
            }
        }
    }
}
#[test]
fn management_fixtures() {
    let rows: Value =
        serde_json::from_str(include_str!("../../../fixtures/viiper/usbip.json")).unwrap();
    assert_eq!(management_reply(5, true), hex(&rows["devlist"]));
    assert_eq!(management_reply(3, true), hex(&rows["import"]));
    assert_eq!(&management_reply(3, false), &[1, 0x11, 0, 3, 0, 0, 0, 1]);
}
