import logging
import shlex
import subprocess
import time
from datetime import datetime, timedelta, timezone

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
        broker_url: str,
        broker_port: int,
    ):
        self._client_name = client_name
        self._connect_timeout = connect_timeout
        self._keepalive = 2 * connect_timeout
        self._mqtt_id: int | str = "Not connected yet"
        self._mqtt_id_timestamp = datetime.now() - timedelta(hours=1)
        self._last_success = datetime.now()
        self._broker_url = broker_url
        self._broker_port = broker_port

        self.async_at = async_at
        self.async_at.add_callback("+CLTS: ", self.set_clock)
        self.async_at.add_callback('+CEREG: 1,"', self.mqtt_connect_callback)
        self.async_at.add_callback("+CMQDISCON:", self.mqtt_connect_callback)
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
            'AT*MCGDEFCONT="IP","trial-nbiot.corp"', timeout=self._connect_timeout
        )
        if not response.success:
            logging.warning("Can not set APN")

    async def power_on(self):
        await self.async_at.call("ATE0", timeout=1)
        res = await self.async_at.call("AT", "OK", timeout=1)
        if res.success:
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

    async def mqtt_disconnect(self, mqtt_id: int | None):
        if mqtt_id is not None:
            await self.async_at.call(f"AT+CMQDISCON={mqtt_id}", timeout=self._keepalive + 10)

    async def _detect_mqtt_id(self) -> int | str:
        self._mqtt_id = "Expired MQTT connection"
        if time_since(self._last_success, timedelta(seconds=self._keepalive)):
            # If there hasn't been a successful send for a long time, do not trust the detection
            return self._mqtt_id
        try:
            response = await self.async_at.call("AT+CMQCON?", f'CMQCON: ([0-9]),1,"{self._broker_url}"')
            if response.query is not None:
                self._mqtt_id = int(response.query[0])
        finally:
            return self._mqtt_id

    async def mqtt_connect_callback(self, s: str):
        self.mqtt_connect()

    async def mqtt_connect(self):
        if time_since(
            self._mqtt_id_timestamp, timedelta(seconds=self._connect_timeout)
        ) and isinstance(await self._detect_mqtt_id(), str):
            await self._mqtt_connect_internal()
            self._mqtt_id_timestamp = datetime.now()

    async def set_clock(self, modem_clock: str):
        tim = is_time_off(modem_clock, datetime.now(timezone.utc))
        if tim is not None:
            subprocess.call(shlex.split(f"sudo -n date -s '{tim.isoformat()}'"))

    async def _mqtt_connect_internal(self) -> int | str:
        await self.async_at.call("ATE0")
        if isinstance(self._mqtt_id, int):
            return self._mqtt_id

        response = await self.async_at.call("AT+CEREG?", "CEREG: [0123],[15]")
        correct = any(line.startswith("+CEREG: 3") for line in response.full_response)
        if not correct:
            await self.async_at.call("AT+CEREG=3")
        if response.query is None:
            self._mqtt_id = "Not registered yet"
            return self._mqtt_id

        response = await self.async_at.call("AT+CCLK?", "CCLK: (.*)")
        if response.query is not None:
            await self.set_clock(response.query[0])

        response = await self.async_at.call(
            "AT+CMQNEW?",
            f"\\+CMQNEW: ([0-9]),1,{self._broker_url}",
        )
        if response.query is not None:
            # CMQNEW is fine but CMQCON is not, the only solution is a disconnect
            await self.mqtt_disconnect(int(response.query[0]))

        response = await self.async_at.call(
            f'AT+CMQNEW="{self._broker_url}","{self._broker_port}",{self._connect_timeout}000,200',
            "CMQNEW: ([0-9])",
            timeout=150,  # Timeout is very long for this command
        )
        if response.query is None:
            await self.async_at.call("AT+CIPPING=8.8.8.8,2,32,50", "OK", timeout=15)
            time.sleep(10)  # 2 pings, 5 seconds each
            self._mqtt_id = "MQTT connection unsuccessful"
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
            else:
                # TODO: Ping here too
                self._mqtt_id = "MQTT connection unsuccessful"
        except Exception as err:
            self._mqtt_id = f"MQTT connection unsuccessful: {err}"

        return self._mqtt_id

    async def mqtt_send(self, topic: str, message: bytes, qos: int = 0) -> bool | str:
        await self.mqtt_connect()

        if isinstance(self._mqtt_id, str):
            if time_since(self._last_success, RESTART_TIME):  # TODO: wrap into a function
                await self.async_at.call("AT+CFUN=0", "", timeout=10)
                await self.async_at.call("AT+CFUN=1", "")
                self._last_success = datetime.now()  # Do not restart too often
            return self._mqtt_id

        message_hex = message.hex()
        response = await self.async_at.call(
            f'AT+CMQPUB={self._mqtt_id},"{topic}",{qos},0,0,{len(message_hex)},"{message_hex}"',
            timeout=self._connect_timeout + 3,
        )
        if response.success:
            self._mqtt_id_timestamp = datetime.now()
            self._last_success = datetime.now()
            return True
        return "MQTT send unsuccessful"

    async def get_signal_info(self) -> tuple[int, int] | None:
        response = await self.async_at.call("AT+CENG?", "CENG: (.*)", [6, 3])
        if self.async_at.last_at_response() < datetime.now() - timedelta(minutes=5):
            await self.power_on()
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
