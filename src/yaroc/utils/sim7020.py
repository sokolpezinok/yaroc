import logging
import shlex
import subprocess
import time
from datetime import datetime, timedelta, timezone
from typing import Callable

from ..pb.status_pb2 import Disconnected, Status
from ..utils.sys_info import is_raspberrypi, is_time_off
from .async_serial import AsyncATCom


def time_since(t: datetime, delta: timedelta) -> bool:
    return datetime.now() - t > delta


RESTART_TIME = timedelta(minutes=40)


class SIM7020Interface:
    """An AT interface to the SIM7020 NB-IoT chip

    Implements mostly MQTT functionality

    Uses pyserial-asyncio under the hood to communicate with the modem.

    Note: this class is not thread-safe.
    """

    def __init__(
        self,
        async_at: AsyncATCom,
        will_topic: str,
        client_name: str,
        connect_timeout: float,
        connection_callback: Callable[[str], None],
        broker_url: str,
        broker_port: int,
    ):
        self._client_name = client_name
        self._connect_timeout = connect_timeout
        self._keepalive = 2 * connect_timeout
        self._mqtt_id: int | None = None
        self._mqtt_id_timestamp = datetime.now() - timedelta(hours=1)
        self._last_success = datetime.now()
        self._broker_url = broker_url
        self._broker_port = broker_port

        self.async_at = async_at
        self.power_on()
        self.async_at.call("ATE0")
        self.async_at.call("AT+CMEE=2")  # Text error messages
        self.async_at.call("AT+CREVHEX=1")  # Hex messages
        self.async_at.call("AT+CLTS=1")  # Synchronize time from network
        self.async_at.add_callback("+CLTS: ", self.set_clock)
        self.async_at.add_callback("+CPIN", lambda x: None)
        self.async_at.add_callback('+CEREG: 1,"', connection_callback)
        self.async_at.add_callback("+CMQDISCON:", connection_callback)
        response = self.async_at.call(
            'AT*MCGDEFCONT="IP","trial-nbiot.corp"', timeout=self._connect_timeout
        )
        if not response.success:
            logging.warning("Can not set APN")
            return None

        status = Status()
        disconnected = Disconnected()
        disconnected.client_name = client_name
        status.disconnected.CopyFrom(disconnected)
        self._will = status.SerializeToString()
        self._will_topic = will_topic

        if not self.async_at.call("AT").success:
            logging.info("SIM7020 is ready")

    def __del__(self):
        mqtt_id = self._detect_mqtt_id()
        self.mqtt_disconnect(mqtt_id)

    def power_on(self):
        self.async_at.call("ATE0", timeout=1)
        if self.async_at.call("AT", "OK", timeout=1).success:
            logging.info("SIM7020 is powered on")
        else:
            if is_raspberrypi():
                logging.info("Powering on SIM7020")
                import RPi.GPIO as GPIO

                POWER_KEY = 4
                GPIO.setmode(GPIO.BCM)
                GPIO.setwarnings(False)
                GPIO.setup(POWER_KEY, GPIO.OUT)
                GPIO.output(POWER_KEY, GPIO.HIGH)
                time.sleep(1)
                GPIO.output(POWER_KEY, GPIO.LOW)
                time.sleep(3)
            else:
                logging.error("Cannot power on the module, press the power button")

    def mqtt_disconnect(self, mqtt_id: int | None):
        if mqtt_id is not None:
            self.async_at.call(f"AT+CMQDISCON={mqtt_id}", timeout=self._keepalive + 10)

    def _detect_mqtt_id(self) -> int | None:
        self._mqtt_id = None
        if time_since(self._last_success, timedelta(seconds=self._keepalive)):
            # If there hasn't been a successful send for a long time, do not trust the detection
            return self._mqtt_id
        try:
            response = self.async_at.call("AT+CMQCON?", f'CMQCON: ([0-9]),1,"{self._broker_url}"')
            if response.query is not None:
                self._mqtt_id = int(response.query[0])
        finally:
            return self._mqtt_id

    def mqtt_connect(self):
        if (
            time_since(self._mqtt_id_timestamp, timedelta(seconds=self._connect_timeout))
            and self._detect_mqtt_id() is None
        ):
            self._mqtt_id = self._mqtt_connect_internal()
            self._mqtt_id_timestamp = datetime.now()

    def set_clock(self, modem_clock: str):
        tim = is_time_off(modem_clock, datetime.now(timezone.utc))
        if tim is not None:
            subprocess.call(shlex.split(f"sudo -n date -s '{tim.isoformat()}'"))

    def _mqtt_connect_internal(self) -> int | None:
        self.async_at.call("ATE0")
        if self._mqtt_id is not None:
            return self._mqtt_id

        response = self.async_at.call("AT+CEREG?", "CEREG: [0123],[15]")
        correct = any(line.startswith("+CEREG: 3") for line in response.full_response)
        if not correct:
            self.async_at.call("AT+CEREG=3")
        if response.query is not None:
            logging.warning("Not registered yet")
            return None

        response = self.async_at.call("AT+CCLK?", "CCLK: (.*)")
        if response.query is not None:
            self.set_clock(response.query[0])

        response = self.async_at.call(
            "AT+CMQNEW?",
            f"\\+CMQNEW: ([0-9]),1,{self._broker_url}",
        )
        if response.query is not None:
            # CMQNEW is fine but CMQCON is not, the only solution is a disconnect
            self.mqtt_disconnect(int(response.query[0]))

        response = self.async_at.call(
            f'AT+CMQNEW="{self._broker_url}","{self._broker_port}",{self._connect_timeout}000,200',
            "CMQNEW: ([0-9])",
            timeout=150,  # Timeout is very long for this command
        )
        if response.query is None:
            self.async_at.call("AT+CIPPING=8.8.8.8,2,32,50", "OK", timeout=15)
            time.sleep(10)  # 2 pings, 5 seconds each
            return None
        try:
            mqtt_id = int(response.query[0])
            will_hex = self._will.hex()
            response = self.async_at.call(
                f'AT+CMQCON={mqtt_id},3,"{self._client_name}",{self._keepalive},0,1,'
                f'"topic={self._will_topic},qos=1,retained=0,'
                f'message_len={len(will_hex)},message={will_hex}"',
                timeout=self._keepalive,
            )
            if response.success:
                logging.info(f"Connected to mqtt_id={mqtt_id}")
                return mqtt_id
            else:
                logging.error("MQTT connection unsuccessful")
                return None
        except Exception as err:
            logging.error(f"MQTT connection unsuccessful: {err}")
            return None

    def mqtt_send(self, topic: str, message: bytes, qos: int = 0) -> bool:
        self.mqtt_connect()

        if self._mqtt_id is None:
            logging.warning("Not connected, will not send an MQTT message")
            if time_since(self._last_success, RESTART_TIME):  # TODO: wrap into a function
                self.async_at.call("AT+CFUN=0", "", timeout=10)
                self.async_at.call("AT+CFUN=1", "")
                self._last_success = datetime.now()  # Do not restart too often
            return False

        message_hex = message.hex()
        response = self.async_at.call(
            f'AT+CMQPUB={self._mqtt_id},"{topic}",{qos},0,0,{len(message_hex)},"{message_hex}"',
            timeout=self._connect_timeout + 3,
        )
        if response.success:
            self._mqtt_id_timestamp = datetime.now()
            self._last_success = datetime.now()
        return response.success

    def get_signal_info(self) -> tuple[int, int] | None:
        response = self.async_at.call("AT+CENG?", "CENG: (.*)", [6, 3])
        if self.async_at.last_at_response() < datetime.now() - timedelta(minutes=5):
            self.power_on()
        try:
            if response.query is not None:
                try:
                    cellid = int(response.query[1][1:-1], 16)
                except Exception:
                    logging.error(f"Failed to parse cell ID {response.query[1]}")
                    return None

                return (int(response.query[0]), cellid)
            return None
        except Exception as err:
            logging.error(f"Error getting signal dBm {err}")
            return None
