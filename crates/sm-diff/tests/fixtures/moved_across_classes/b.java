package demo;

class Reader {
    void open() {
        stream.open();
    }

    void read(byte[] into) {
        stream.read(into);
    }
}

class Writer {
    void close() {
        stream.close();
    }

    void write(byte[] out) {
        stream.write(out);
    }

    int checksum(byte[] data) {
        return Crc.of(data);
    }
}
