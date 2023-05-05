import asyncio
import logging
from collections.abc import Callable
from concurrent.futures import Future, ThreadPoolExecutor
from datetime import datetime, time, timedelta
from queue import Queue
from threading import Condition, Thread
from typing import Any, Generic, Optional, Tuple, TypeVar

T = TypeVar("T")
A = TypeVar("A")


class BackoffRetries(Generic[A, T]):
    """
    A sender that does exponential backoff in case of failed send operations

    The class is thread-safe
    """

    def __init__(
        self,
        send_function: Callable[[A], T],
        on_publish: Callable[[Any], Any],
        first_backoff: float,
        multiplier: float,
        max_duration: timedelta,
    ):
        self.send_function = send_function
        self.on_publish = on_publish
        self.first_backoff = first_backoff
        self.max_duration = max_duration
        self.multiplier = multiplier

        def start_background_loop(loop: asyncio.AbstractEventLoop) -> None:
            asyncio.set_event_loop(loop)
            loop.run_forever()

        self._loop = asyncio.new_event_loop()
        self._thread = Thread(target=start_background_loop, args=(self._loop,), daemon=True)
        self._thread.start()

    async def _backoff_send(self, argument: A) -> Optional[T]:
        deadline = datetime.now() + self.max_duration
        cur_backoff = self.first_backoff

        while datetime.now() < deadline:
            try:
                ret = self.send_function(argument)
                try:
                    self.on_publish(argument)
                finally:
                    return ret
            except Exception as e:
                logging.error(e)
                if datetime.now() + timedelta(seconds=cur_backoff) >= deadline:
                    cur_backoff = (deadline - datetime.now()).total_seconds()
                logging.info(f"Retrying after {cur_backoff} seconds")
                await asyncio.sleep(cur_backoff)
                cur_backoff = cur_backoff * self.multiplier

        logging.info(f"Message expired, args = {argument}")
        return None

    def close(self, timeout=None):
        self._thread.join(timeout)

    def send(self, argument: A) -> Future:
        future = asyncio.run_coroutine_threadsafe(self._backoff_send(argument), self._loop)
        logging.debug("Scheduled")  # TODO: add message ID
        return future


class BatchMessageInfo:
    def __init__(self, mid: int):
        self._published = False
        self._condition = Condition()
        self.mid = mid

    def set_as_published(self):
        with self._condition:
            self._published = True
            self._condition.notify_all()

    def wait_for_publish(self, timeout=None):
        timeout_time = None if timeout is None else time.time() + timeout
        timeout_tenth = None if timeout is None else timeout / 10.0

        def timed_out():
            return False if timeout_time is None else time.time() > timeout_time

        with self._condition:
            while not self._published and not timed_out():
                self._condition.wait(timeout_tenth)


class BatchRetries(Generic[A, T]):
    # TODO: add callback and max_duration
    def __init__(self, send_function: Callable[[list[A]], T], batch_count: int = 1):
        self.queue: Queue[Tuple[A, BatchMessageInfo]] = Queue()
        self.send_function = send_function
        self.executor = ThreadPoolExecutor(max_workers=1)
        self.batch_count = batch_count
        self.message_counter = 0

    def _send_remaining(self):
        messages, message_infos = [], []
        while not self.queue.empty():
            message, message_info = self.queue.get()
            messages.append(message)
            message_infos.append(message_info)
            if len(messages) >= self.batch_count:
                break
        if len(messages) == 0:
            return

        try:
            self.send_function(messages)
            for message_info in message_infos:
                message_info.set_as_published()
        except Exception:
            for message, message_info in zip(messages, message_infos):
                self.queue.put((message, message_info))

    def _send_all(self, _):
        if not self.queue.empty():
            self.executor.submit(self._send_remaining).add_done_callback(self._send_all)

    def send(self, arg: A) -> BatchMessageInfo:
        ret = BatchMessageInfo(self.message_counter)
        self.message_counter += 1
        self.queue.put((arg, ret))
        self.executor.submit(self._send_remaining).add_done_callback(self._send_all)
        return ret

    def execute(self, fn, *args) -> Future:
        return self.executor.submit(fn, *args)
