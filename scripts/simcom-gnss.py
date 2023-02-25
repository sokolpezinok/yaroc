#!/usr/bin/env python3
# Works for SIMCom modules with GNSS capabilities

import serial
import time
from datetime import datetime
import logging
import socket

ser = serial.Serial("/dev/ttyUSB5", 9600)
ser.flushInput()

powerKey = 4
time_count = 0


logging.basicConfig(
    encoding="utf-8",
    level=logging.DEBUG,
    format="%(asctime)s - %(levelname)s - %(message)s",
)

def sendAt(command, back, timeout):
    rec_buff = b''
    ser.write((command + "\r\n").encode())
    time.sleep(timeout)
    if ser.inWaiting():
        time.sleep(0.01)
        rec_buff = ser.read(ser.inWaiting())
    res = rec_buff.decode()
    if back not in res:
        logging.debug(command + " back:\t" + res)
        return (0, res)
    else:
        logging.debug(res)
        return (1, res)


def getGpsPosition():
    logging.debug("Starting GPS session...")
    (answer, res) = sendAt("AT+CSQ", "OK", 0.1)
    if answer == 1:
        with open("/home/lukas/csq.log", "a") as f:
            f.write(f"{datetime.now()}:{res}\n")

    sendAt("AT+CGNSPWR=1", "OK", 0.1)
    sendAt("AT+CGNSCOLD", "OK", 3)
    while True:
        (answer, res) = sendAt("AT+CGNSINF", "+CGNSINF: ", 13)
        if 1 == answer:
            if "0.000000" in res or ",,,,,,,," in res:
                logging.debug(res)
                logging.error("GPS is not ready")
            else:
                response = res.split("+CGNSINF:")
                if len(response) >= 2:
                    sendAt("AT+CGNSPWR=0", "OK", 3)
                    raw_coords = response[1].split(',')
                    coords = list(map(float, raw_coords[3:6]))
                    message = f"{coords[0]};{coords[1]};{coords[2]};{datetime.now()}"
                    log_message = f"{coords[0]},{coords[1]} ({coords[2]} alt, at {datetime.now()})"
                    logging.info(log_message)
                    with open("/home/lukas/gps.log", "a") as f:
                        f.write(log_message + "\n")

                    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                    sock.connect(("127.0.0.1", 12345))
                    try:
                        sock.sendall(bytes(message, encoding='utf-8'))
                    finally:
                        sock.close()
                    return True
        else:
            logging.error("AT command failed")
            sendAt("AT+CGNSPWR=0", "OK", 3)
            return False
        time.sleep(2.5)


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


try:
    checkStart()
    getGpsPosition()
except:
    if ser != None:
        ser.close()
