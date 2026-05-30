import asyncio
import logging
import time
from asyncio import Queue
from dataclasses import dataclass
from datetime import datetime
from typing import AsyncIterator

from ..rs import Event, MessageHandler, SiPunch

DEFAULT_TIMEOUT_MS = 3.0
START_MODE = 3
FINISH_MODE = 4
BEACON_CONTROL = 18
SI_LABS = "10c4"


@dataclass
class DeviceEvent:
    added: bool
    device: str


class SiWorker:
    def __init__(self) -> None:
        self._codes: set[int] = set()

    async def process_punch(self, punch: SiPunch, queue: Queue[SiPunch]):
        now = datetime.now().astimezone()
        logging.info(
            f"{punch.card} punched {punch.code} at {punch.time:%H:%M:%S.%f}, received after "
            f"{(now - punch.time).total_seconds():3.2f}s"
        )
        await queue.put(punch)
        self._codes.add(punch.code)

    @property
    def codes(self) -> set[int]:
        return self._codes


class UdevSiFactory(SiWorker):
    def __init__(
        self, enable_meshtastic: bool = False, dns: list[tuple[str, str]] | None = None
    ) -> None:
        super().__init__()
        self.enable_meshtastic = enable_meshtastic
        self.dns = dns if dns is not None else []

    async def loop(self, queue: Queue[SiPunch], status_queue: Queue[DeviceEvent]):
        self.handler, self.usb_serial_manager = MessageHandler.new(
            self.dns, [], enable_meshtastic=self.enable_meshtastic, enable_sportident=True
        )
        await asyncio.gather(
            self.usb_serial_manager.loop(),
            self.get_punches(queue, status_queue),
        )

    async def get_punches(self, queue: Queue[SiPunch], status_queue: Queue[DeviceEvent]):
        while True:
            try:
                ev = await self.handler.next_event()
                match ev:
                    case Event.SiPunch():  # type: ignore
                        await self.process_punch(ev[0], queue)
                    case Event.SiPunchLogs():
                        for punch_log in ev[0]:
                            await self.process_punch(punch_log.punch, queue)
                    case Event.DeviceEvnt():
                        await status_queue.put(DeviceEvent(ev.added, ev.device))
                    case Event.MeshtasticLog():
                        logging.info(ev[0])
                    case Event.CellularLog():
                        logging.info(ev[0])
            except Exception as e:
                logging.error(f"Error while getting punches: {e}")


class FakeSiWorker(SiWorker):
    """Creates fake SportIdent events, useful for benchmarks and tests."""

    def __init__(self, punch_interval_secs: float | None = None):
        super().__init__()
        self.name = "fake"
        self._punch_interval = punch_interval_secs if punch_interval_secs is not None else 12.0
        logging.info(
            "Starting a fake SportIdent worker, sending a punch every "
            f"{self._punch_interval} seconds"
        )

    def __hash__(self):
        return "fake".__hash__()

    async def loop(self, queue: Queue, _status_queue):
        del _status_queue
        while True:
            time_start = time.time()
            now = datetime.now().astimezone()
            punch = SiPunch.new(46283, 47, now, 18)
            await self.process_punch(punch, queue)
            await asyncio.sleep(self._punch_interval - (time.time() - time_start))


class SiPunchManager:
    """
    Manages devices delivering SportIdent punch data, typically SRR dongles but also punches
    delivered by radio (LoRa) or Bluetooth.

    Also issues an event whenever a devices has been connected or removed.
    """

    def __init__(self, workers: list[SiWorker]) -> None:
        self._si_workers: set[SiWorker] = set(workers)
        self._queue: Queue[SiPunch] = Queue()
        self._status_queue: Queue[DeviceEvent] = Queue()

    async def loop(self):
        loops = []
        for worker in self._si_workers:
            self._si_workers.add(worker)
            loops.append(worker.loop(self._queue, self._status_queue))
        await asyncio.sleep(3)  # Allow some time for an MQTT connection
        await asyncio.gather(*loops, return_exceptions=True)

    async def punches(self) -> AsyncIterator[SiPunch]:
        while True:
            yield await self._queue.get()

    async def device_events(self) -> AsyncIterator[DeviceEvent]:
        while True:
            yield await self._status_queue.get()

    @property
    def codes(self) -> set[int]:
        worker_codes = [worker.codes for worker in self._si_workers]
        return set().union(*worker_codes)
