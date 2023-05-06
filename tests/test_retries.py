import time
import unittest
from datetime import datetime, timedelta

from yaroc.utils.retries import BackoffRetries, BatchRetries


class TestRetries(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
