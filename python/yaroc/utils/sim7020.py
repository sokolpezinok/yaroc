import asyncio
import logging
import shlex
import subprocess
import time
from datetime import datetime, timedelta, timezone
from typing import TypeAlias

from ..pb.status_pb2 import Disconnected, Status
from ..utils.sys_info import RaspberryModel, is_time_off, raspberrypi_model
from .async_serial import AsyncATCom

ErrStr: TypeAlias = str


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
        broker_url: str,
        broker_port: int,
    ):
        self._client_name = client_name
        self._connect_timeout = connect_timeout
        self._keepalive = 2 * connect_timeout
        self._mqtt_id: int | ErrStr = "Not connected yet"
        self._mqtt_id_timestamp: datetime = datetime.now() - timedelta(hours=1)
        self._last_success = datetime.now()
        self._broker_url = broker_url
        self._broker_port = broker_port
        self._state_lock = asyncio.Lock()

        self.async_at = async_at
        self.async_at.add_callback("+CLTS:", self.mqtt_connect_callback)
        self.async_at.add_callback('+CEREG: 1,"', self.mqtt_connect_callback)
        self.async_at.add_callback("+CMQDISCON:", self.mqtt_disconnect_callback)
        self.async_at.add_callback("*MGCOUNT:", self.counter_callback)
        status = Status()
        disconnected = Disconnected()
        disconnected.client_name = client_name
        status.disconnected.CopyFrom(disconnected)
        self._will = status.SerializeToString()
        self._will_topic = will_topic

    def __del__(self):
        pass
        # TODO: reenable
        # mqtt_id = self._detect_mqtt_id()
        # self.mqtt_disconnect(mqtt_id)

    async def setup(self):
        await self.power_on()
        await self.async_at.call("ATE0")
        await self.async_at.call("AT+CMEE=2")  # Text error messages
        await self.async_at.call("AT+CREVHEX=1")  # Hex messages
        await self.async_at.call("AT+CMQTSYNC=1")  # Synchronous MQTT
        await self.async_at.call("AT+CLTS=1")  # Synchronize time from network
        response = await self.async_at.call(
            'AT*MCGDEFCONT="IP","internet.iot"', timeout=self._connect_timeout
        )

        if not response.success:
            logging.warning("Can not set APN")

    async def power_on(self):
        await self.async_at.call("ATE0", timeout=1)
        res = await self.async_at.call("AT", "OK", timeout=1)
        if raspberrypi_model() == RaspberryModel.Unknown:
            logging.error("Cannot power on the module, press the power button")
        else:
            import RPi.GPIO as GPIO

            POWER_KEY = 4
            GPIO.setmode(GPIO.BCM)
            GPIO.setwarnings(False)
            GPIO.setup(POWER_KEY, GPIO.OUT)
            if res.success:
                logging.info("SIM7020 is powered on")
            else:
                logging.info("Powering on SIM7020")
                GPIO.output(POWER_KEY, GPIO.HIGH)
                time.sleep(1)
                GPIO.output(POWER_KEY, GPIO.LOW)
                time.sleep(5)

    async def mqtt_disconnect(self, mqtt_id: int | None):
        if mqtt_id is not None:
            await self.async_at.call(f"AT+CMQDISCON={mqtt_id}", timeout=self._keepalive + 10)

    async def _detect_mqtt_id(self) -> int | ErrStr:
        # Connection made recently
        if not time_since(self._mqtt_id_timestamp, timedelta(seconds=self._connect_timeout)):
            return self._mqtt_id
        # Last successful send a long time ago, not trusting the modem
        if time_since(self._last_success, timedelta(seconds=self._keepalive)):
            logging.warn("Too long since a successful send, force a reconnect")
            self._mqtt_id = ErrStr("Expired MQTT connection")
            return self._mqtt_id
        try:
            if isinstance(self._mqtt_id, ErrStr):
                response = await self.async_at.call(
                    "AT+CMQCON?", f'CMQCON: ([0-9]),1,"{self._broker_url}"'
                )
                if response.query is not None:
                    self._mqtt_id = int(response.query[0])
        finally:
            return self._mqtt_id

    async def mqtt_connect_callback(self, s: str):
        await self.mqtt_connect()

    async def mqtt_disconnect_callback(self, s: str):
        self._mqtt_id = ErrStr("Disconnected")
        await self.mqtt_connect()

    async def counter_callback(self, s: str):
        try:
            parsed = list(map(int, s.split(",")[:5]))
            _, _, uu, _, du = parsed
            logging.debug(f"Uploaded: {uu} bytes, downloaded: {du} bytes")
        except Exception as err:
            logging.error(f"Failed to parse {s} as counters: {err}")

    async def mqtt_connect(self):
        async with self._state_lock:
            if isinstance(await self._detect_mqtt_id(), ErrStr):
                await self._mqtt_connect_internal()
                if isinstance(self._mqtt_id, ErrStr):
                    logging.error(f"MQTT connection failed: {self._mqtt_id}")

    async def set_clock(self, modem_clock: str):
        tim = is_time_off(modem_clock, datetime.now(timezone.utc))
        if tim is not None:
            subprocess.call(shlex.split(f"sudo -n date -s '{tim.isoformat()}'"))

    async def ping(self):
        await self.async_at.call("AT+CIPPING=8.8.8.8,1,32,130", "OK", timeout=15)

    async def _mqtt_connect_internal(self) -> int | ErrStr:
        await self.async_at.call("ATE0")
        if isinstance(self._mqtt_id, int):
            return self._mqtt_id

        response = await self.async_at.call("AT+CEREG?", "CEREG: [0123],[15]")
        correct = any(line.startswith("+CEREG: 3") for line in response.full_response)
        if not correct:
            await self.async_at.call("AT+CEREG=3")
        if response.query is None:
            self._mqtt_id = ErrStr("Not registered yet")
            return self._mqtt_id

        response = await self.async_at.call("AT+CCLK?", "CCLK: (.*)")
        if response.query is not None:
            await self.set_clock(response.query[0])

        response = await self.async_at.call("AT+CMQNEW?", "\\+CMQNEW: ([0-9]),1")
        if response.query is not None:
            # CMQNEW is fine but CMQCON is not, the only solution is a disconnect
            await self.mqtt_disconnect(int(response.query[0]))

        response = await self.async_at.call(
            f'AT+CMQNEW="{self._broker_url}","{self._broker_port}",{self._connect_timeout}000,400',
            "CMQNEW: ([0-9])",
            timeout=153,  # Timeout is very long for this command
        )
        if response.query is None:
            await self.ping()
            self._mqtt_id = ErrStr("Connection AT command unsuccessful")
            return self._mqtt_id
        try:
            mqtt_id = int(response.query[0])
            will_hex = self._will.hex()
            response = await self.async_at.call(
                f'AT+CMQCON={mqtt_id},3,"{self._client_name}",{self._keepalive},0,1,'
                f'"topic={self._will_topic},qos=1,retained=0,'
                f'message_len={len(will_hex)},message={will_hex}"',
                timeout=self._keepalive,
            )
            if response.success:
                logging.info(f"Connected to mqtt_id={mqtt_id}")
                self._mqtt_id = mqtt_id
                self._mqtt_id_timestamp = datetime.now()
            else:
                await self.ping()
                self._mqtt_id = ErrStr("Connection unsuccessful")
        except Exception as err:
            self._mqtt_id = ErrStr(f"{err}")

        return self._mqtt_id

    async def restart_modem(self):
        await self.async_at.call("AT+CFUN=0", "", timeout=10)
        await self.async_at.call("AT+CFUN=1", "")
        self._last_success = datetime.now()  # Do not restart too often

    async def mqtt_send(self, topic: str, message: bytes, qos: int = 0) -> bool | ErrStr:
        await self.mqtt_connect()

        if isinstance(self._mqtt_id, ErrStr):
            if time_since(self._last_success, RESTART_TIME):
                logging.info("Too long since the last successful MQTT send, restarting modem")
                await self.restart_modem()
            return self._mqtt_id

        message_hex = message.hex()
        response = await self.async_at.call(
            f'AT+CMQPUB={self._mqtt_id},"{topic}",{qos},0,0,{len(message_hex)},"{message_hex}"',
            timeout=self._connect_timeout + 3,
        )
        if response.success:
            self._last_success = datetime.now()
            self._mqtt_id_timestamp = datetime.now()
            return True
        return "MQTT send unsuccessful"

    async def get_signal_info(self) -> tuple[int, int, int, int] | None:
        await self.async_at.call("AT*MGCOUNT=1,1")
        response = await self.async_at.call("AT+CENG?", "CENG: (.*)", [6, 3, 7, 10])
        if self.async_at.last_at_response() < datetime.now() - timedelta(minutes=5):
            await self.power_on()
        try:
            if response.query is not None:
                try:
                    cellid = int(response.query[1][1:-1], 16)
                except Exception:
                    logging.error(f"Failed to parse cell ID {response.query[1]}")
                    return None

                return (
                    int(response.query[0]),
                    cellid,
                    int(response.query[2]),
                    int(response.query[3]),
                )
            return None
        except Exception as err:
            logging.error(f"Error getting signal dBm {err}")
            return None
