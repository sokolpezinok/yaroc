# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# source: src/yaroc/pb/punches.proto
"""Generated protocol buffer code."""
from google.protobuf.internal import builder as _builder
from google.protobuf import descriptor as _descriptor
from google.protobuf import descriptor_pool as _descriptor_pool
from google.protobuf import symbol_database as _symbol_database
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()


from google.protobuf import timestamp_pb2 as google_dot_protobuf_dot_timestamp__pb2


DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(b'\n\x1asrc/yaroc/pb/punches.proto\x1a\x1fgoogle/protobuf/timestamp.proto\"-\n\x05Punch\x12\x0b\n\x03raw\x18\x01 \x01(\x0c\x12\x17\n\x0fprocess_time_ms\x18\x02 \x01(\r\"Y\n\x07Punches\x12\x17\n\x07punches\x18\x01 \x03(\x0b\x32\x06.Punch\x12\x35\n\x11sending_timestamp\x18\x02 \x01(\x0b\x32\x1a.google.protobuf.Timestampb\x06proto3')

_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, globals())
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, 'src.yaroc.pb.punches_pb2', globals())
if _descriptor._USE_C_DESCRIPTORS == False:

  DESCRIPTOR._options = None
  _PUNCH._serialized_start=63
  _PUNCH._serialized_end=108
  _PUNCHES._serialized_start=110
  _PUNCHES._serialized_end=199
# @@protoc_insertion_point(module_scope)
