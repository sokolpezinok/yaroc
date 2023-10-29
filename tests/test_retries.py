import asyncio
import time
import unittest
from datetime import datetime, timedelta

from yaroc.utils.retries import BackoffBatchedRetries, BackoffRetries


class TestBackoffRetries(unittest.IsolatedAsyncioTestCase):
    async def test_backoff_retries(self):
        # Note: this is not the best test as it is non-deterministic, but the error-margin is
        # pretty wide
        stats = {2: 0, 4: 0}

        async def send_f(x: int) -> datetime | None:
            await asyncio.sleep(0.025)
            stats[x] += 1
            if stats[x] < x:
                return None
            return datetime.now()

        b = BackoffRetries(send_f, None, 0.04, 2.0, timedelta(minutes=0.1))

        async def sleep_and_4():
            await asyncio.sleep(0.08)
            return await b.backoff_send(4)

        start = datetime.now()
        [finished2, finished4] = await asyncio.gather(b.backoff_send(2), sleep_and_4())
        published4 = datetime.now()

        self.assertAlmostEqual(
            finished2.timestamp(),
            (start + timedelta(seconds=0.11)).timestamp(),
            delta=0.03,
        )
        self.assertAlmostEqual(
            finished4.timestamp(),
            (start + timedelta(seconds=0.49)).timestamp(),
            delta=0.03,
        )
        self.assertAlmostEqual(finished4.timestamp(), published4.timestamp(), delta=0.004)


class TestBatchedBackoffRetries(unittest.IsolatedAsyncioTestCase):
    async def test_backoff_batched_retries(self):
        # Note: this is not the best test as it is non-deterministic, but the error-margin is
        # pretty wide
        stats = {1: 0, 2: 0, 3: 0}

        def send_f(xs: list[int]) -> list[datetime | None]:
            ret: list[datetime | None] = []
            for x in xs:
                time.sleep(0.04)
                if stats[x] < x:
                    stats[x] += 1
                    ret.append(None)
                else:
                    ret.append(datetime.now())
            return ret

        b = BackoffBatchedRetries(send_f, None, 0.03, 2.0, timedelta(seconds=10), batch_count=2)

        async def sleep_and_1():
            await asyncio.sleep(0.004)
            return await b.send(1)

        async def sleep_and_2():
            await asyncio.sleep(0.002)
            return await b.send(2)

        start = datetime.now()
        [finished1, finished2, finished3] = await asyncio.gather(
            sleep_and_1(), sleep_and_2(), b.send(3)
        )

        self.assertAlmostEqual(
            finished1.timestamp(),
            (start + timedelta(seconds=0.242)).timestamp(),
            delta=0.05,
        )
        self.assertAlmostEqual(
            finished2.timestamp(),
            (start + timedelta(seconds=0.342)).timestamp(),
            delta=0.05,
        )
        self.assertAlmostEqual(
            finished3.timestamp(),
            (start + timedelta(seconds=0.445)).timestamp(),
            delta=0.05,
        )


if __name__ == "__main__":
    unittest.main()
