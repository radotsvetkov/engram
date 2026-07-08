#!/usr/bin/env python3
"""arduino_sketch_scaffold — Engram skill (no network). Generate a ready-to-flash
Arduino .ino sketch for common hardware patterns.

Supported pattern values: led_blink, button_read, servo_control,
ultrasonic_sensor, analog_read. Each produces a real setup()/loop() sketch
with sensible default pin numbers when 'pin' isn't given.

Request (stdin): {"pattern": "led_blink", "pin": 13}
Output (stdout): {filename: "sketch.ino", code: str}
"""
import json
import sys

SUPPORTED = ("led_blink", "button_read", "servo_control", "ultrasonic_sensor", "analog_read")

DEFAULT_PINS = {
    "led_blink": 13,
    "button_read": 2,
    "servo_control": 9,
    "ultrasonic_sensor": 9,   # trig pin; echo defaults to 10
    "analog_read": "A0",
}


def _led_blink(pin):
    return """const int LED_PIN = %s;

void setup() {
  pinMode(LED_PIN, OUTPUT);
}

void loop() {
  digitalWrite(LED_PIN, HIGH);
  delay(1000);
  digitalWrite(LED_PIN, LOW);
  delay(1000);
}
""" % pin


def _button_read(pin):
    return """const int BUTTON_PIN = %s;

void setup() {
  pinMode(BUTTON_PIN, INPUT_PULLUP);
  Serial.begin(9600);
}

void loop() {
  // INPUT_PULLUP means the pin reads LOW when the button is pressed.
  int state = digitalRead(BUTTON_PIN);
  if (state == LOW) {
    Serial.println("Button pressed");
  } else {
    Serial.println("Button released");
  }
  delay(100);
}
""" % pin


def _servo_control(pin):
    return """#include <Servo.h>

const int SERVO_PIN = %s;
Servo myServo;

void setup() {
  myServo.attach(SERVO_PIN);
}

void loop() {
  for (int angle = 0; angle <= 180; angle += 1) {
    myServo.write(angle);
    delay(15);
  }
  for (int angle = 180; angle >= 0; angle -= 1) {
    myServo.write(angle);
    delay(15);
  }
}
""" % pin


def _ultrasonic_sensor(trig_pin):
    echo_pin = trig_pin + 1
    return """const int TRIG_PIN = %d;
const int ECHO_PIN = %d;

void setup() {
  pinMode(TRIG_PIN, OUTPUT);
  pinMode(ECHO_PIN, INPUT);
  Serial.begin(9600);
}

void loop() {
  // Send a 10us pulse to trigger the HC-SR04 ultrasonic sensor.
  digitalWrite(TRIG_PIN, LOW);
  delayMicroseconds(2);
  digitalWrite(TRIG_PIN, HIGH);
  delayMicroseconds(10);
  digitalWrite(TRIG_PIN, LOW);

  // Measure the echo pulse duration and convert to distance.
  long duration = pulseIn(ECHO_PIN, HIGH);
  float distanceCm = duration * 0.0343 / 2.0;

  Serial.print("Distance: ");
  Serial.print(distanceCm);
  Serial.println(" cm");
  delay(500);
}
""" % (trig_pin, echo_pin)


def _analog_read(pin):
    return """const int ANALOG_PIN = %s;

void setup() {
  Serial.begin(9600);
}

void loop() {
  int reading = analogRead(ANALOG_PIN);
  Serial.print("Analog reading: ");
  Serial.println(reading);
  delay(200);
}
""" % pin


BUILDERS = {
    "led_blink": _led_blink,
    "button_read": _button_read,
    "servo_control": _servo_control,
    "ultrasonic_sensor": _ultrasonic_sensor,
    "analog_read": _analog_read,
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"pattern": "led_blink"}}))
        return 0

    pattern = q.get("pattern")
    if pattern not in SUPPORTED:
        print(json.dumps({
            "error": "'pattern' must be one of: %s" % ", ".join(SUPPORTED),
            "example": {"pattern": "led_blink"},
        }))
        return 0

    pin = q.get("pin")
    if pin is None:
        pin = DEFAULT_PINS[pattern]
    elif pattern == "ultrasonic_sensor":
        # This pattern needs an int (echo pin = trig pin + 1).
        try:
            pin = int(pin)
        except (TypeError, ValueError):
            print(json.dumps({"error": "'pin' must be an integer for pattern 'ultrasonic_sensor'"}))
            return 0
    elif pattern != "analog_read":
        try:
            pin = int(pin)
        except (TypeError, ValueError):
            print(json.dumps({"error": "'pin' must be an integer for pattern %r" % pattern}))
            return 0

    try:
        code = BUILDERS[pattern](pin)
    except Exception as e:
        print(json.dumps({"error": "sketch generation failed: %s" % e}))
        return 1

    print(json.dumps({
        "filename": "sketch.ino",
        "code": code,
    }, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
