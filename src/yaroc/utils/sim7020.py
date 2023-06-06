import logging
import shlex
import subprocess
import time
from datetime import datetime, timedelta, timezone
from typing import Callable

from ..pb.status_pb2 import Disconnected, Status
from .async_serial import AsyncATCom

# TODO: either share these constants or make them parameters
BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883
CONNECT_TIME = 45


def time_since(t: datetime, delta: timedelta) -> bool:
    return datetime.now() - t > delta


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
        registration_callback: Callable[[str], None],
    ):
        self._client_name = client_name
        self._default_delay = 100
        self._default_timeout = 1
        self._mqtt_id: int | None = None
        self._mqtt_id_timestamp = datetime.now()
        self._last_success = datetime.now()

        status = Status()
        disconnected = Disconnected()
        disconnected.client_name = client_name
        status.disconnected.CopyFrom(disconnected)
        self._will = status.SerializeToString()
        self._will_topic = will_topic

        self.async_at = async_at
        self.async_at.call("ATE0")
        self.async_at.call("AT+CMEE=2")
        self.async_at.call("AT+CREVHEX=1")
        self.async_at.add_callback("+CLTS", lambda x: None)
        self.async_at.add_callback("+CPIN", lambda x: None)
        self.async_at.add_callback('+CGREG: 1,"', registration_callback)

        if self.async_at.call("AT") is not None:
            logging.info("SIM7020 is ready")

    def __del__(self):
        mqtt_id = self._detect_mqtt_id()
        self.mqtt_disconnect(mqtt_id)

    def mqtt_disconnect(self, mqtt_id: int | None):
        if mqtt_id is not None:
            self.async_at.call(f"AT+CMQDISCON={mqtt_id}", timeout=2 * CONNECT_TIME)

    def _detect_mqtt_id(self) -> int | None:
        self._mqtt_id = None
        if time_since(self._last_success, timedelta(minutes=3)):
            # If there hasn't been a successful send for a long time, do not trust the detection
            return self._mqtt_id
        try:
            response = self.async_at.call("AT+CMQCON?", f'CMQCON: ([0-9]),1,"{BROKER_URL}"')
            if response.query is not None:
                self._mqtt_id = int(response.query[0])
        finally:
            return self._mqtt_id

    def mqtt_connect(self):
        self._mqtt_id = self._mqtt_connect_internal()
        self._mqtt_id_timestamp = datetime.now()

    def set_clock(self, clock: str):
        try:
            tim = (
                datetime.strptime(clock, "%y/%m/%d,%H:%M:%S+08")
                .replace(tzinfo=timezone.utc)
                .astimezone()
            )
            if (tim - datetime.utcnow()) > timedelta(seconds=5):
                subprocess.call(shlex.split(f"sudo -n date -s '{tim.isoformat()}'"))
                time.sleep(5)
        except Exception as err:
            logging.error(f"Failed to set time: {err}")

    def _mqtt_connect_internal(self) -> int | None:
        self.async_at.call("ATE0")
        self.async_at.call("AT+CGREG=2")

        self._detect_mqtt_id()
        if self._mqtt_id is not None:
            return self._mqtt_id

        if self.async_at.call("AT+CGREG?", "CGREG: [012],([0-9])") is None:
            logging.warning("Not registered yet")
            return None

        # Can APN be set automatically?
        response = self.async_at.call('AT*MCGDEFCONT="IP","trial-nbiot.corp"', timeout=CONNECT_TIME)
        if response is None:
            logging.warning("Can not set APN")
            return None

        response = self.async_at.call("AT+CCLK?", "CCLK: (.*)", None)
        if response.query is not None:
            self.set_clock(response.query[0])

        response = self.async_at.call(
            "AT+CMQNEW?",
            f"\\+CMQNEW: ([0-9]),1,{BROKER_URL}",
        )
        if response.query is not None:
            # CMQNEW is fine but CMQCON is not, the only solution is a disconnect
            self.mqtt_disconnect(int(response.query[0]))

        response = self.async_at.call(
            f'AT+CMQNEW="{BROKER_URL}","{BROKER_PORT}",{CONNECT_TIME}000,200',
            "CMQNEW: ([0-9])",
            timeout=150,  # Timeout is very long for this command
        )
        if response.query is None:
            return None
        try:
            mqtt_id = int(response.query[0])
            will_hex = self._will.hex()
            response = self.async_at.call(
                f'AT+CMQCON={mqtt_id},3,"{self._client_name}",{2 * CONNECT_TIME},0,1,'
                f'"topic={self._will_topic},qos=1,retained=0,'
                f'message_len={len(will_hex)},message={will_hex}"',
                timeout=2 * CONNECT_TIME,
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
        if (qos == 1 and time_since(self._mqtt_id_timestamp, timedelta(seconds=30))) or (
            qos == 0 and time_since(self._mqtt_id_timestamp, timedelta(minutes=3))
        ):
            if self._detect_mqtt_id() is None:
                self.mqtt_connect()

        if self._mqtt_id is None:
            logging.warning("Not connected, will not send an MQTT message")
            if time_since(self._last_success, timedelta(minutes=15)):
                self.async_at.call("AT+CFUN=0", "", timeout=10)
                self.async_at.call("AT+CFUN=1", "")
                self._last_success = datetime.now()  # Do not restart too often
            return False

        message_hex = message.hex()
        response = self.async_at.call(
            f'AT+CMQPUB={self._mqtt_id},"{topic}",{qos},0,0,{len(message_hex)},"{message_hex}"',
            timeout=CONNECT_TIME + 3,
        )
        if response.success:
            self._mqtt_id_timestamp = datetime.now()
            self._last_success = datetime.now()
        return response.success

    def get_signal_dbm(self) -> int | None:
        response = self.async_at.call("AT+CENG?", "CENG: (.*)", 6)
        try:
            if response.query is not None:
                return int(response.query[0])
            return None
        except Exception as err:
            logging.error(f"Error getting signal dBm {err}")
            return None
