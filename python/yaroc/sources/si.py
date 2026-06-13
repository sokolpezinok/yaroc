import asyncio
import logging
from asyncio import Queue
from dataclasses import dataclass
from datetime import datetime, timedelta
from typing import AsyncIterator

from ..rs import Event, MessageHandlerBuilder, SiPunch

DEFAULT_TIMEOUT_MS = 3.0
START_MODE = 3
FINISH_MODE = 4
BEACON_CONTROL = 18


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
        self,
        enable_sportident: bool = True,
        enable_meshtastic: bool = False,
        meshtastic_tcp: str | None = None,
        dns: list[tuple[str, str]] | None = None,
        fake_punch_interval: float | None = None,
    ) -> None:
        super().__init__()
        self.enable_sportident = enable_sportident
        self.enable_meshtastic = enable_meshtastic
        self.meshtastic_tcp = meshtastic_tcp
        self.dns = dns if dns is not None else []
        self.fake_punch_interval = fake_punch_interval

    async def loop(self, queue: Queue[SiPunch], status_queue: Queue[DeviceEvent]):
        builder = (
            MessageHandlerBuilder()
            .with_dns(self.dns)
            .with_meshtastic(self.enable_meshtastic)
            .with_sportident(self.enable_sportident)
        )
        if self.meshtastic_tcp is not None:
            builder = builder.with_tcp(self.meshtastic_tcp)
        if self.fake_punch_interval is not None:
            builder = builder.with_fake_punch(timedelta(seconds=self.fake_punch_interval))
        self.handler, self.usb_serial_manager = builder.build()
        await asyncio.gather(
            self.usb_serial_manager.loop(),
            self.get_punches(queue, status_queue),
        )

    async def get_punches(self, queue: Queue[SiPunch], status_queue: Queue[DeviceEvent]):
        while True:
            try:
                ev = await self.handler.next_event()
                match ev:
                    case Event.SiPunch():
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
