import asyncio
import logging
from asyncio import Condition, Lock, Queue
from collections.abc import Awaitable, Callable
from datetime import datetime, timedelta
from typing import Generic, Optional, TypeVar

T = TypeVar("T")
A = TypeVar("A")


# TODO: consider using https://pypi.org/project/backoff/
class BackoffRetries(Generic[A, T]):
    """
    A sender that does exponential backoff in case of failed send operations

    The class is thread-safe
    """

    def __init__(
        self,
        send_function: Callable[[A], Awaitable[T]],
        failed_outcome: T,
        first_backoff: float,
        multiplier: float,
        max_duration: timedelta,
    ):
        self.send_function = send_function
        self.first_backoff = first_backoff
        self.max_duration = max_duration
        self.multiplier = multiplier
        self.failed_outcome = failed_outcome
        self._current_mid = 0

    async def backoff_send(self, argument: A) -> T:
        self._current_mid += 1
        mid = self._current_mid
        logging.debug(f"Scheduled: {mid}")

        deadline = datetime.now() + self.max_duration
        cur_backoff = self.first_backoff
        while datetime.now() < deadline:
            try:
                ret = await self.send_function(argument)
                if ret != self.failed_outcome:
                    logging.info(f"Sent: {mid}")
                    return ret
            except Exception as err:
                logging.error(f"Sending failed: {err}")

            if datetime.now() + timedelta(seconds=cur_backoff) >= deadline:
                cur_backoff = (deadline - datetime.now()).total_seconds()
                if cur_backoff < 0:
                    break
            logging.error(f"Message not sent: mid={mid}, retrying after {cur_backoff} seconds")
            await asyncio.sleep(cur_backoff)
            cur_backoff = cur_backoff * self.multiplier

        logging.error(f"Message mid={mid} expired, args = {argument}")
        return self.failed_outcome


class RetriedMessage(Generic[A, T]):
    def __init__(self, arg: A, mid: int):
        self.processed = Condition()
        self.returned: T | None = None
        self.mid = mid
        self.arg = arg

    async def set_published(self, returned: T):
        async with self.processed:
            self.returned = returned
            self.processed.notify()

    async def set_not_published(self):
        async with self.processed:
            self.processed.notify()


class BackoffBatchedRetries(Generic[A, T]):
    """
    A sender that does exponential backoff in case of failed send operations

    The class is thread-safe
    """

    def __init__(
        self,
        send_function: Callable[[list[A]], Awaitable[list[T]]],
        failed_outcome: T,
        first_backoff: float,
        multiplier: float,
        max_duration: timedelta,
        batch_count: int = 2,
        workers: int = 1,
    ):
        self.send_function = send_function
        self.first_backoff = first_backoff
        self.max_duration = max_duration
        self.multiplier = multiplier
        self.batch_count = batch_count
        self.failed_outcome = failed_outcome
        self._lock = Lock()
        self._queue: Queue[RetriedMessage] = Queue()
        self._current_mid_lock = Lock()
        self._current_mid = 0

    async def _send_and_notify(self):
        messages = []
        async with self._lock:
            while not self._queue.empty() and len(messages) < self.batch_count:
                messages.append(self._queue.get_nowait())
            if len(messages) == 0:
                return ([], [])

            returned = await self.send_function([message.arg for message in messages])

        published, not_published = [], []
        for message, r in zip(messages, returned):
            if r == self.failed_outcome:
                await message.set_not_published()
                not_published.append(message.mid)
            else:
                await message.set_published(r)
                published.append(message.mid)

        if len(published) > 0:
            logging.info("Messages sent: " + ",".join(map(str, published)))
        if len(not_published) > 0:
            logging.error("Messages not sent: " + ",".join(map(str, not_published)))

    async def send(self, argument: A) -> Optional[T]:
        async with self._current_mid_lock:
            self._current_mid += 1
            retried_message: RetriedMessage[A, T] = RetriedMessage(argument, self._current_mid)
        logging.debug(f"Scheduled: mid={retried_message.mid}")

        deadline = datetime.now() + self.max_duration
        cur_backoff = self.first_backoff
        while datetime.now() < deadline:
            async with retried_message.processed:
                await self._queue.put(retried_message)
                asyncio.create_task(self._send_and_notify())
                await retried_message.processed.wait()
                if retried_message.returned is not None:
                    return retried_message.returned

            if datetime.now() + timedelta(seconds=cur_backoff) >= deadline:
                cur_backoff = (deadline - datetime.now()).total_seconds()
                if cur_backoff < 0:
                    break
            logging.info(f"Retrying mid={retried_message.mid} after {cur_backoff} seconds")
            await asyncio.sleep(cur_backoff)
            cur_backoff = cur_backoff * self.multiplier

        logging.error(f"Message mid={retried_message.mid} expired, args = {argument}")
        return None
