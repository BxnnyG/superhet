# Applied to apps/m17-demod.cpp of m17-cxx-demod by the Dockerfile (stage `m17`): after the LSF
# line "SRC: ..., DEST: ..., STR:V/V CAN:00" also print the raw 16-bit TYPE field in hex. Upstream
# prints only the stream/packet bit, the data type and the CAN; the encryption bits (3..6) that
# tell an encrypted call from a clear one and the META contents are not visible otherwise.
# The plugin adapter reads ", TYPE: 0xNNNN" when it is there and copes when it is not.
/^[[:space:]]*dump_type(type);/a\
        std::cerr << ", TYPE: 0x" << std::hex << std::setw(4) << std::setfill((char)48) << type << std::dec;
