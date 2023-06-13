import asyncio
import time
import unittest
from datetime import datetime, timedelta

from yaroc.utils.retries import BackoffBatchedRetries, BackoffRetries


class TestBackoffRetries(unittest.TestCase):
    def test_backoff_retries(self):
        # Note: this is not the best test as it is non-deterministic, but the error-margin is
        # pretty wide
        stats = {2: 0, 4: 0}

        async def send_f(x: int) -> datetime | None:
            await asyncio.sleep(0.025)
            stats[x] += 1
            if stats[x] < x:
                return None
            return datetime.now()

        b = BackoffRetries(send_f, 0.04, 2.0, timedelta(minutes=0.1))
        start = datetime.now()
        f2 = b.send(2)
        time.sleep(0.13)
        f4 = b.send(4)
        finished2 = f2.result()
        finished4 = f4.result()
        published4 = datetime.now()
        b.close(0.1)

        self.assertAlmostEqual(
            finished2.timestamp(),
            (start + timedelta(seconds=0.09)).timestamp(),
            delta=0.004,
        )
        self.assertAlmostEqual(
            finished4.timestamp(),
            (start + timedelta(seconds=0.51)).timestamp(),
            delta=0.008,
        )
        self.assertAlmostEqual(finished4.timestamp(), published4.timestamp(), delta=0.004)

    def test_backoff_batched_retries(self):
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

        b = BackoffBatchedRetries(send_f, 0.03, 2.0, timedelta(minutes=0.1), batch_count=2)
        start = datetime.now()
        f3 = b.send(3)
        time.sleep(0.002)
        f2 = b.send(2)
        time.sleep(0.002)
        f1 = b.send(1)
        finished1 = f1.result()
        finished2 = f2.result()
        finished3 = f3.result()
        b.close(0.1)

        self.assertAlmostEqual(
            finished1.timestamp(),
            (start + timedelta(seconds=0.242)).timestamp(),
            delta=0.01,
        )
        self.assertAlmostEqual(
            finished2.timestamp(),
            (start + timedelta(seconds=0.342)).timestamp(),
            delta=0.01,
        )
        self.assertAlmostEqual(
            finished3.timestamp(),
            (start + timedelta(seconds=0.445)).timestamp(),
            delta=0.01,
        )


if __name__ == "__main__":
    unittest.main()
