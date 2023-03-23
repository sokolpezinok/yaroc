import unittest
import xml.etree.ElementTree as ET
from datetime import timedelta

from yaroc.clients.mop import MOP, MeosCategory, MeosResult

TEST_XML = """<?xml version="1.0" encoding="UTF-8"?>
<MOPComplete xmlns="http://www.melin.nu/mop" nextdifference="1377871">
  <competition date="2023-03-12" organizer="" homepage="">Training</competition>
  <ctrl id="74">74-1</ctrl>
  <ctrl id="100074">74-2</ctrl>
  <cls id="1" ord="10" radio="74,100074,200074">A</cls>
  <cls id="2" ord="40" radio="74">C</cls>
  <org id="22" nat="SVK">Klub OB Sokol Pezinok</org>
  <cmp id="165" card="2078195">
    <base org="22" cls="2" stat="1" st="484570" rt="29800">Sara Doe</base>
    <radio>74,25220</radio>
    <input it="0" tstat="1" />
  </cmp>
  <cmp id="168" card="2111071">
    <base org="22" cls="2" stat="4" st="-1" rt="0">John Doe</base>
    <input it="0" tstat="1" />
  </cmp>
</MOPComplete>
"""


class TestMeos(unittest.TestCase):
    def test_xml_parsing(self):
        xml = ET.XML(TEST_XML)
        ET.indent(xml)
        results = MOP._meos_results_xml(xml)
        self.assertEqual(
            results[0],
            MeosResult(
                category=MeosCategory(name="C", id="2"),
                name="Sara Doe",
                card=2078195,
                stat=1,
                time=timedelta(seconds=2980),
            ),
        )
        self.assertEqual(
            results[1],
            MeosResult(category=MeosCategory(name='C', id='2'), name='John Doe', card=2111071,
                       stat=4, time=None),
        )


if __name__ == "__main__":
    unittest.main()
