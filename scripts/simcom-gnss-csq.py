#!/usr/bin/env python3
# Works for SIMCom modules with GNSS capabilities

import sys
from attila.atre import ATRuntimeEnvironment
from attila.exceptions import (
    ATRuntimeError,
    ATScriptSyntaxError,
    ATSerialPortError,
)
import time
from datetime import datetime
import logging

atrunenv = ATRuntimeEnvironment(False)
atrunenv.configure_communicator("/dev/ttyUSB2", 9600, 30, "\r\n")
try:
    atrunenv.open_serial()
except ATSerialPortError:
    logging.error("Failed to open serial port")
    time.sleep(10)
    sys.exit(1)

logging.basicConfig(
    encoding="utf-8",
    level=logging.DEBUG,
    format="%(asctime)s - %(levelname)s - %(message)s",
)


def send_at(command: str, queries=[str]):
    try:
        opt_response = atrunenv.exec(command)
    except ATRuntimeError as err:
        logging.error(f"Runtime error {err}")
        return None
    except ATScriptSyntaxError as err:
        logging.error(f"Syntax error {err}")
        return None
    except ATSerialPortError:
        logging.error("Failed to open serial port")
        return None
    except:
        return None

    logging.debug(f"{opt_response.full_response}: {opt_response.response}")
    if opt_response is None or opt_response.response is None:
        return None
    response = opt_response
    res = []
    for query in queries:
        res.append(response.get_collectable(query))
    return res


def getCsq():
    ret = send_at('AT+CSQ;;CSQ;;0;;5;;["CSQ: ?{rssi::[0-9]+},99"]', ["rssi"])
    if ret is None:
        return 0
    return int(ret[0])


def getGpsPosition():
    logging.debug("Starting GPS session...")

    send_at("AT+CGNSPWR=1;;OK;;0;;0.1")
    send_at("AT+CGNSCOLD;;OK;;200;;1")
    for _ in range(10):
        (answer, res) = send_at("AT+CGNSINF", "+CGNSINF: ")
        if 1 == answer:
            if "0.000000" in res or ",,,,,,,," in res:
                logging.warning("GPS is not ready")
            else:
                response = res.split("+CGNSINF:")
                if len(response) >= 2:
                    send_at("AT+CGNSPWR=0", "OK", 3)
                    raw_coords = response[1].split(",")
                    return list(map(float, raw_coords[3:6]))
        else:
            logging.error("AT command failed")
            send_at("AT+CGNSPWR=0", "OK")
            return None
        time.sleep(2.5)
    logging.error("GPS did not work in time")
    send_at("AT+CGNSPWR=0", "OK", 3)
    return None


def checkStart():
    while True:
        atrunenv.exec("ATE")
        atrunenv.exec("AT")
        response = atrunenv.exec("AT").full_response
        if "OK" in response:
            logging.info("SOM7080X is ready")
            break


def sendMqttMessages(messages):
    send_at("AT+CPSI?;;OK;;0;;1")
    res = send_at("AT+CGREG?;;CGREG: 0,1;;")
    if res is None:
        logging.warning("Not connected yet")
        csq = getCsq()
        logging.info(f"CSQ: {csq}")
        time.sleep(5)
        return
    send_at('AT+CNCFG=0,1,"trial-nbiot.corp";;OK')
    send_at("AT+CNACT=0,1;;OK;;200;;30")
    send_at('AT+SMCONF="URL",18.193.153.59,1883;;OK;;1000')
    send_at('AT+SMCONF="KEEPTIME",60;;OK')
    send_at('AT+SMCONF="CLIENTID","47";;OK')
    send_at('AT+SMCONF="TOPIC","spe/47";;OK')
    send_at("AT+SMCONN;;OK;;3000;;25")
    send_at(";;;;20000;;1")

    csq = getCsq()
    if csq is not None:
        messages.append(f"{csq};{datetime.now()}")
        logging.info(f"CSQ {csq} at {datetime.now()}")
        with open("/home/lukas/events.log", "a") as f:
            f.write(f"{datetime.now()}: CSQ {csq}, {-114 + 2*csq} dBm\n")

    for message in messages:
        send_at(f'AT+SMPUB="spe/47",{len(message)},1,0;;>;;0;;1')
        send_at(f"{message};;;;1000;;20")
        send_at(";;;;2000;;10")

    send_at("AT+SMDISC;;OK;;2000;;5")
    send_at("AT+CNACT=0,0;;OK")


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
finally:
    atrunenv.close_serial()
