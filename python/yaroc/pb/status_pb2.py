# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# source: python/yaroc/pb/status.proto
"""Generated protocol buffer code."""
from google.protobuf.internal import builder as _builder
from google.protobuf import descriptor as _descriptor
from google.protobuf import descriptor_pool as _descriptor_pool
from google.protobuf import symbol_database as _symbol_database
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()


from google.protobuf import timestamp_pb2 as google_dot_protobuf_dot_timestamp__pb2


DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n\x1cpython/yaroc/pb/status.proto\x1a\x1fgoogle/protobuf/timestamp.proto\"#\n\x0c\x44isconnected\x12\x13\n\x0b\x63lient_name\x18\x01 \x01(\t\"5\n\x0b\x44\x65viceEvent\x12\x0c\n\x04port\x18\x01 \x01(\t\x12\x18\n\x04type\x18\x02 \x01(\x0e\x32\n.EventType\"n\n\x0b\x43oordinates\x12\x10\n\x08latitude\x18\x01 \x01(\x02\x12\x11\n\tlongitude\x18\x02 \x01(\x02\x12\x10\n\x08\x61ltitude\x18\x03 \x01(\x02\x12(\n\x04time\x18\x04 \x01(\x0b\x32\x1a.google.protobuf.Timestamp\"\x97\x02\n\x0cMiniCallHome\x12\x10\n\x08local_ip\x18\x02 \x01(\r\x12\x17\n\x0f\x63pu_temperature\x18\x03 \x01(\x02\x12\x0c\n\x04\x66req\x18\x04 \x01(\r\x12\x10\n\x08min_freq\x18\x05 \x01(\r\x12\x10\n\x08max_freq\x18\x06 \x01(\r\x12\r\n\x05volts\x18\x07 \x01(\x02\x12\x12\n\nsignal_dbm\x18\x08 \x01(\x05\x12\x0e\n\x06\x63\x65llid\x18\t \x01(\r\x12\x14\n\x0cnetwork_type\x18\n \x01(\r\x12\r\n\x05\x63odes\x18\x0b \x01(\t\x12\x13\n\x0btotaldatarx\x18\x0c \x01(\x04\x12\x13\n\x0btotaldatatx\x18\r \x01(\x04\x12(\n\x04time\x18\x0e \x01(\x0b\x32\x1a.google.protobuf.Timestamp\"\x82\x01\n\x06Status\x12%\n\x0c\x64isconnected\x18\x01 \x01(\x0b\x32\r.DisconnectedH\x00\x12\'\n\x0emini_call_home\x18\x02 \x01(\x0b\x32\r.MiniCallHomeH\x00\x12!\n\tdev_event\x18\x03 \x01(\x0b\x32\x0c.DeviceEventH\x00\x42\x05\n\x03msg*0\n\tEventType\x12\x0b\n\x07Unknown\x10\x00\x12\t\n\x05\x41\x64\x64\x65\x64\x10\x01\x12\x0b\n\x07Removed\x10\x02\x62\x06proto3')

_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, globals())
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'python.yaroc.pb.status_pb2', globals())
if _descriptor._USE_C_DESCRIPTORS == False:

  DESCRIPTOR._options = None
  _EVENTTYPE._serialized_start=684
  _EVENTTYPE._serialized_end=732
  _DISCONNECTED._serialized_start=65
  _DISCONNECTED._serialized_end=100
  _DEVICEEVENT._serialized_start=102
  _DEVICEEVENT._serialized_end=155
  _COORDINATES._serialized_start=157
  _COORDINATES._serialized_end=267
  _MINICALLHOME._serialized_start=270
  _MINICALLHOME._serialized_end=549
  _STATUS._serialized_start=552
  _STATUS._serialized_end=682
# @@protoc_insertion_point(module_scope)
