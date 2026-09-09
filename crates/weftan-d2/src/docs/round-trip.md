# Lossless round trips and field ownership

The adapter keeps the parsed D2 document as a generic JSON tree while building
a separate engine graph. This is the core losslessness rule: decoding a field
for layout does not discard neighboring fields the engine does not understand.
