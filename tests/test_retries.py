import asyncio
import time
import unittest
from datetime import datetime, timedelta

from yaroc.utils.retries import BackoffBatchedRetries, BackoffRetries, RetriedMessage


class TestBackoffRetries(unittest.TestCase):
    def test_backoff_retries(self):
        # Note: this is not the best test as it is non-deterministic, but the error-margin is
        # pretty wide
        stats = {2: 0, 4: 0}

        def send_f(x: int) -> datetime:
            time.sleep(0.025)
            stats[x] += 1
            if stats[x] < x:
                raise Exception(f"Failed arg={x} for the {stats[x]}th time")
            return datetime.now()

        b = BackoffRetries(send_f, lambda x: x, 0.04, 2.0, timedelta(minutes=0.1))
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

        def send_f(xs: list[int]):
            one_failed = False
            for x in xs:
                time.sleep(0.04)
                if stats[x] < x:
                    stats[x] += 1
                    one_failed = True
            if one_failed:
                raise Exception(f"Failed {xs}")
            return [datetime.now()] * len(xs)

        b = BackoffBatchedRetries(
            send_f, lambda x: x, 0.03, 2.0, timedelta(minutes=0.1), batch_count=2
        )
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

        self.assertAlmostEqual(finished2.timestamp(), finished1.timestamp(), delta=0.001)
        self.assertAlmostEqual(
            finished2.timestamp(),
            (start + timedelta(seconds=0.385)).timestamp(),
            delta=0.01,
        )
        self.assertAlmostEqual(
            finished3.timestamp(),
            (start + timedelta(seconds=0.445)).timestamp(),
            delta=0.01,
        )


if __name__ == "__main__":
    unittest.main()
