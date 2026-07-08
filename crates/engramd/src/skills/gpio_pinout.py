#!/usr/bin/env python3
"""gpio_pinout — Engram skill (no network). Look up the well-known static
pinout for a common single-board computer or microcontroller dev board.

Request (stdin): {"board": "raspberry_pi_40pin"}
  board is one of: raspberry_pi_40pin, arduino_uno, arduino_nano, esp32_devkit
Output (stdout): {board, pin_count, pinout: [{pin_number, name, function}, ...], note}
"""
import json
import sys

_PI_40 = [
    {"pin_number": 1, "name": "3V3", "function": "3.3V power"},
    {"pin_number": 2, "name": "5V", "function": "5V power"},
    {"pin_number": 3, "name": "GPIO2", "function": "I2C1 SDA"},
    {"pin_number": 4, "name": "5V", "function": "5V power"},
    {"pin_number": 5, "name": "GPIO3", "function": "I2C1 SCL"},
    {"pin_number": 6, "name": "GND", "function": "Ground"},
    {"pin_number": 7, "name": "GPIO4", "function": "GPCLK0"},
    {"pin_number": 8, "name": "GPIO14", "function": "UART TXD"},
    {"pin_number": 9, "name": "GND", "function": "Ground"},
    {"pin_number": 10, "name": "GPIO15", "function": "UART RXD"},
    {"pin_number": 11, "name": "GPIO17", "function": "General purpose I/O"},
    {"pin_number": 12, "name": "GPIO18", "function": "PCM_CLK / PWM0"},
    {"pin_number": 13, "name": "GPIO27", "function": "General purpose I/O"},
    {"pin_number": 14, "name": "GND", "function": "Ground"},
    {"pin_number": 15, "name": "GPIO22", "function": "General purpose I/O"},
    {"pin_number": 16, "name": "GPIO23", "function": "General purpose I/O"},
    {"pin_number": 17, "name": "3V3", "function": "3.3V power"},
    {"pin_number": 18, "name": "GPIO24", "function": "General purpose I/O"},
    {"pin_number": 19, "name": "GPIO10", "function": "SPI0 MOSI"},
    {"pin_number": 20, "name": "GND", "function": "Ground"},
    {"pin_number": 21, "name": "GPIO9", "function": "SPI0 MISO"},
    {"pin_number": 22, "name": "GPIO25", "function": "General purpose I/O"},
    {"pin_number": 23, "name": "GPIO11", "function": "SPI0 SCLK"},
    {"pin_number": 24, "name": "GPIO8", "function": "SPI0 CE0"},
    {"pin_number": 25, "name": "GND", "function": "Ground"},
    {"pin_number": 26, "name": "GPIO7", "function": "SPI0 CE1"},
    {"pin_number": 27, "name": "GPIO0", "function": "ID_SD (HAT EEPROM)"},
    {"pin_number": 28, "name": "GPIO1", "function": "ID_SC (HAT EEPROM)"},
    {"pin_number": 29, "name": "GPIO5", "function": "General purpose I/O"},
    {"pin_number": 30, "name": "GND", "function": "Ground"},
    {"pin_number": 31, "name": "GPIO6", "function": "General purpose I/O"},
    {"pin_number": 32, "name": "GPIO12", "function": "PWM0"},
    {"pin_number": 33, "name": "GPIO13", "function": "PWM1"},
    {"pin_number": 34, "name": "GND", "function": "Ground"},
    {"pin_number": 35, "name": "GPIO19", "function": "PCM_FS / PWM1"},
    {"pin_number": 36, "name": "GPIO16", "function": "General purpose I/O"},
    {"pin_number": 37, "name": "GPIO26", "function": "General purpose I/O"},
    {"pin_number": 38, "name": "GPIO20", "function": "PCM_DIN"},
    {"pin_number": 39, "name": "GND", "function": "Ground"},
    {"pin_number": 40, "name": "GPIO21", "function": "PCM_DOUT"},
]

_UNO = [
    {"pin_number": 1, "name": "D0", "function": "Digital I/O 0 / UART RX"},
    {"pin_number": 2, "name": "D1", "function": "Digital I/O 1 / UART TX"},
    {"pin_number": 3, "name": "D2", "function": "Digital I/O 2 / external interrupt INT0"},
    {"pin_number": 4, "name": "D3", "function": "Digital I/O 3 (PWM) / external interrupt INT1"},
    {"pin_number": 5, "name": "D4", "function": "Digital I/O 4"},
    {"pin_number": 6, "name": "D5", "function": "Digital I/O 5 (PWM)"},
    {"pin_number": 7, "name": "D6", "function": "Digital I/O 6 (PWM)"},
    {"pin_number": 8, "name": "D7", "function": "Digital I/O 7"},
    {"pin_number": 9, "name": "D8", "function": "Digital I/O 8"},
    {"pin_number": 10, "name": "D9", "function": "Digital I/O 9 (PWM)"},
    {"pin_number": 11, "name": "D10", "function": "Digital I/O 10 (PWM) / SPI SS"},
    {"pin_number": 12, "name": "D11", "function": "Digital I/O 11 (PWM) / SPI MOSI"},
    {"pin_number": 13, "name": "D12", "function": "Digital I/O 12 / SPI MISO"},
    {"pin_number": 14, "name": "D13", "function": "Digital I/O 13 / SPI SCK, onboard LED"},
    {"pin_number": 15, "name": "A0", "function": "Analog input 0"},
    {"pin_number": 16, "name": "A1", "function": "Analog input 1"},
    {"pin_number": 17, "name": "A2", "function": "Analog input 2"},
    {"pin_number": 18, "name": "A3", "function": "Analog input 3"},
    {"pin_number": 19, "name": "A4", "function": "Analog input 4 / I2C SDA"},
    {"pin_number": 20, "name": "A5", "function": "Analog input 5 / I2C SCL"},
    {"pin_number": 21, "name": "VIN", "function": "Unregulated supply voltage input"},
    {"pin_number": 22, "name": "5V", "function": "Regulated 5V power output"},
    {"pin_number": 23, "name": "3.3V", "function": "Regulated 3.3V power output"},
    {"pin_number": 24, "name": "GND", "function": "Ground"},
    {"pin_number": 25, "name": "GND", "function": "Ground"},
    {"pin_number": 26, "name": "RESET", "function": "Active-low reset"},
    {"pin_number": 27, "name": "IOREF", "function": "Reference voltage for shields"},
    {"pin_number": 28, "name": "AREF", "function": "Analog reference voltage"},
]

_NANO = [
    {"pin_number": 1, "name": "D0", "function": "Digital I/O 0 / UART RX"},
    {"pin_number": 2, "name": "D1", "function": "Digital I/O 1 / UART TX"},
    {"pin_number": 3, "name": "D2", "function": "Digital I/O 2 / external interrupt INT0"},
    {"pin_number": 4, "name": "D3", "function": "Digital I/O 3 (PWM) / external interrupt INT1"},
    {"pin_number": 5, "name": "D4", "function": "Digital I/O 4"},
    {"pin_number": 6, "name": "D5", "function": "Digital I/O 5 (PWM)"},
    {"pin_number": 7, "name": "D6", "function": "Digital I/O 6 (PWM)"},
    {"pin_number": 8, "name": "D7", "function": "Digital I/O 7"},
    {"pin_number": 9, "name": "D8", "function": "Digital I/O 8"},
    {"pin_number": 10, "name": "D9", "function": "Digital I/O 9 (PWM)"},
    {"pin_number": 11, "name": "D10", "function": "Digital I/O 10 (PWM) / SPI SS"},
    {"pin_number": 12, "name": "D11", "function": "Digital I/O 11 (PWM) / SPI MOSI"},
    {"pin_number": 13, "name": "D12", "function": "Digital I/O 12 / SPI MISO"},
    {"pin_number": 14, "name": "D13", "function": "Digital I/O 13 / SPI SCK, onboard LED"},
    {"pin_number": 15, "name": "A0", "function": "Analog input 0"},
    {"pin_number": 16, "name": "A1", "function": "Analog input 1"},
    {"pin_number": 17, "name": "A2", "function": "Analog input 2"},
    {"pin_number": 18, "name": "A3", "function": "Analog input 3"},
    {"pin_number": 19, "name": "A4", "function": "Analog input 4 / I2C SDA"},
    {"pin_number": 20, "name": "A5", "function": "Analog input 5 / I2C SCL"},
    {"pin_number": 21, "name": "A6", "function": "Analog input 6 (analog-only, no digital function)"},
    {"pin_number": 22, "name": "A7", "function": "Analog input 7 (analog-only, no digital function)"},
    {"pin_number": 23, "name": "5V", "function": "Regulated 5V power output"},
    {"pin_number": 24, "name": "3V3", "function": "Regulated 3.3V power output"},
    {"pin_number": 25, "name": "VIN", "function": "Unregulated supply voltage input"},
    {"pin_number": 26, "name": "GND", "function": "Ground"},
    {"pin_number": 27, "name": "GND", "function": "Ground"},
    {"pin_number": 28, "name": "RESET", "function": "Active-low reset"},
]

_ESP32 = [
    {"pin_number": 1, "name": "EN", "function": "Enable / reset (active low)"},
    {"pin_number": 2, "name": "GPIO36", "function": "Input only (ADC1_CH0, labeled VP)"},
    {"pin_number": 3, "name": "GPIO39", "function": "Input only (ADC1_CH3, labeled VN)"},
    {"pin_number": 4, "name": "GPIO34", "function": "Input only (ADC1_CH6)"},
    {"pin_number": 5, "name": "GPIO35", "function": "Input only (ADC1_CH7)"},
    {"pin_number": 6, "name": "GPIO32", "function": "General purpose I/O (ADC1_CH4)"},
    {"pin_number": 7, "name": "GPIO33", "function": "General purpose I/O (ADC1_CH5)"},
    {"pin_number": 8, "name": "GPIO25", "function": "General purpose I/O (ADC2_CH8, DAC1)"},
    {"pin_number": 9, "name": "GPIO26", "function": "General purpose I/O (ADC2_CH9, DAC2)"},
    {"pin_number": 10, "name": "GPIO27", "function": "General purpose I/O (ADC2_CH7)"},
    {"pin_number": 11, "name": "GPIO14", "function": "General purpose I/O, strapping pin (ADC2_CH6)"},
    {"pin_number": 12, "name": "GPIO12", "function": "Strapping pin (boot voltage select) — avoid pulling high at boot"},
    {"pin_number": 13, "name": "GPIO13", "function": "General purpose I/O (ADC2_CH4)"},
    {"pin_number": 14, "name": "GPIO9", "function": "Often wired to internal SPI flash — avoid using"},
    {"pin_number": 15, "name": "GPIO10", "function": "Often wired to internal SPI flash — avoid using"},
    {"pin_number": 16, "name": "3V3", "function": "3.3V power"},
    {"pin_number": 17, "name": "GND", "function": "Ground"},
    {"pin_number": 18, "name": "VIN", "function": "5V power input (from USB or external supply)"},
    {"pin_number": 19, "name": "GPIO23", "function": "General purpose I/O (default SPI MOSI)"},
    {"pin_number": 20, "name": "GPIO22", "function": "General purpose I/O (default I2C SCL)"},
    {"pin_number": 21, "name": "GPIO1", "function": "UART0 TX (used for flashing/console output)"},
    {"pin_number": 22, "name": "GPIO3", "function": "UART0 RX (used for flashing/console input)"},
    {"pin_number": 23, "name": "GPIO21", "function": "General purpose I/O (default I2C SDA)"},
    {"pin_number": 24, "name": "GPIO19", "function": "General purpose I/O (default SPI MISO)"},
    {"pin_number": 25, "name": "GPIO18", "function": "General purpose I/O (default SPI SCK)"},
    {"pin_number": 26, "name": "GPIO5", "function": "Strapping pin (SPI CS on some boot modes)"},
    {"pin_number": 27, "name": "GPIO17", "function": "General purpose I/O"},
    {"pin_number": 28, "name": "GPIO16", "function": "General purpose I/O"},
    {"pin_number": 29, "name": "GPIO4", "function": "General purpose I/O (ADC2_CH0)"},
    {"pin_number": 30, "name": "GPIO0", "function": "Boot mode select / BOOT button, strapping pin"},
    {"pin_number": 31, "name": "GPIO2", "function": "Strapping pin, often tied to the onboard LED"},
    {"pin_number": 32, "name": "GPIO15", "function": "Strapping pin (ADC2_CH3)"},
    {"pin_number": 33, "name": "GND", "function": "Ground"},
]

_BOARDS = {
    "raspberry_pi_40pin": _PI_40,
    "arduino_uno": _UNO,
    "arduino_nano": _NANO,
    "esp32_devkit": _ESP32,
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"board": "raspberry_pi_40pin"},
        })); return 0

    board = q.get("board")
    if board not in _BOARDS:
        print(json.dumps({
            "error": "missing or unsupported 'board' %r" % (board,),
            "supported_boards": sorted(_BOARDS),
            "example": {"board": "raspberry_pi_40pin"},
        })); return 0

    try:
        pinout = _BOARDS[board]
        result = {
            "board": board,
            "pin_count": len(pinout),
            "pinout": pinout,
            "note": "Verify against your specific board revision's datasheet before wiring.",
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "gpio_pinout failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
