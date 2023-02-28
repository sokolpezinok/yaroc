#!/usr/bin/env python3
# Works for SIMCom modules with GNSS capabilities
# TODO: https://pypi.org/project/attila/

import re
import serial
import time
from datetime import datetime
import logging
import socket

ser = serial.Serial("/dev/ttyUSB5", 9600, timeout=20, write_timeout=25)
ser.flushInput()

powerKey = 4
time_count = 0


logging.basicConfig(
    encoding="utf-8",
    level=logging.DEBUG,
    format="%(asctime)s - %(levelname)s - %(message)s",
)


def sendAt(command, back, timeout):
    rec_buff = b""
    ser.write((command + "\r\n").encode())
    time.sleep(timeout)
    if ser.inWaiting():
        time.sleep(0.01)
        rec_buff = ser.read(ser.inWaiting())
    res = rec_buff.decode().strip()
    if back not in res:
        logging.debug(command + " back:\t" + res)
        return (0, res)
    else:
        logging.debug(res)
        return (1, res)


def getCsq():
    (answer, res) = sendAt("AT+CSQ", "OK", 0.1)
    if answer == 1:
        result = re.search(r"(\d+)", res)
        if len(result.groups()) == 0:
            return None
        return int(result.group(0))
    return None


def getGpsPosition():
    logging.debug("Starting GPS session...")

    sendAt("AT+CGNSPWR=1", "OK", 0.1)
    sendAt("AT+CGNSCOLD", "OK", 3)
    for _ in range(10):
        (answer, res) = sendAt("AT+CGNSINF", "+CGNSINF: ", 13)
        if 1 == answer:
            if "0.000000" in res or ",,,,,,,," in res:
                logging.warning("GPS is not ready")
            else:
                response = res.split("+CGNSINF:")
                if len(response) >= 2:
                    sendAt("AT+CGNSPWR=0", "OK", 3)
                    raw_coords = response[1].split(",")
                    return list(map(float, raw_coords[3:6]))
        else:
            logging.error("AT command failed")
            sendAt("AT+CGNSPWR=0", "OK", 3)
            return None
        time.sleep(2.5)
    logging.error("GPS did not work in time")
    sendAt("AT+CGNSPWR=0", "OK", 3)
    return None


def checkStart():
    while True:
        # simcom module uart may be fool,so it is better to send much times when it starts.
        ser.write("AT\r\n".encode())
        time.sleep(4)
        ser.write("AT\r\n".encode())
        time.sleep(1)
        if ser.inWaiting():
            time.sleep(0.01)
            recBuff = ser.read(ser.inWaiting())
            logging.info("SOM7080X is ready")
            logging.debug("Trying to start" + recBuff.decode())
            if "OK" in recBuff.decode():
                recBuff = ""
                break


def sendMqttMessages(messages):
    time.sleep(5)
    sendAt("AT+CPSI?", "OK", 0.5)
    sendAt("AT+CGREG?", "+CGREG: 0,1", 0.2)
    sendAt('AT+CNCFG=0,1,"trial-nbiot.corp"', "OK", 2)
    sendAt("AT+CNACT=0,1", "OK", 2)
    sendAt('AT+SMCONF="URL",18.193.153.59,1883', "OK", 0.1)
    sendAt('AT+SMCONF="KEEPTIME",60', "OK", 0.1)
    sendAt('AT+SMCONF="CLIENTID","47"', "OK", 0.1)
    sendAt('AT+SMCONF="TOPIC","spe/47"', "OK", 0.1)
    sendAt("AT+SMCONN", "OK", 25)

    csq = getCsq()
    if csq is not None:
        messages.append(f"{csq};{datetime.now()}")
        logging.info(f"CSQ {csq} at {datetime.now()}")
        with open("/home/lukas/events.log", "a") as f:
            f.write(f"{datetime.now()}: CSQ {csq}, {-114 + 2*csq} dBm\n")

    for message in messages:
        raw_message = message.encode()
        sendAt(f'AT+SMPUB="spe/47",{len(raw_message)},1,0', ">", 1)
        ser.write(raw_message)
        time.sleep(10)

    sendAt("AT+SMDISC", "OK", 1)
    sendAt("AT+CNACT=0,0", "OK", 1)


try:
    checkStart()
    messages = []
    # coords = getGpsPosition()
    # Note: turn off GPS as it's killing other functionality
    coords = None
    if coords is not None:
        messages.append(f"{coords[0]};{coords[1]};{coords[2]};{datetime.now()}")
        log_message = (
            f"{datetime.now()}: {coords[0]},{coords[1]}, " f"altitude {coords[2]}"
        )
        logging.info(log_message)
        with open("/home/lukas/events.log", "a") as f:
            f.write(f"{log_message}\n")

    sendMqttMessages(messages)
except:
    if ser != None:
        ser.close()
