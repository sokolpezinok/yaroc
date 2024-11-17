import unittest

from yaroc.rs import RaspberryModel


class TestRpiModel(unittest.TestCase):
    def test_match(self):
        model = RaspberryModel.from_string("Raspberry Pi 2 Model B Rev 1.1")
        self.assertEqual(model, RaspberryModel.V2B)
