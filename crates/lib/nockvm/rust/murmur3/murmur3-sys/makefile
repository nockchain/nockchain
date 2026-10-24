CFLAGS = -O3 -Wall

.PHONY: all clean tests

shared: murmur3.c murmur3.h
	$(CC) -fPIC -O3 -c murmur3.c -o $(OUT_DIR)/murmur3.o
	$(CC) -shared -Wl,--export-dynamic $(OUT_DIR)/murmur3.o -o $(OUT_DIR)/libmurmur3.so

# -Wl,-export-dynamic is supposed to work on macOS but doesn't on ld64 XCode 16.0 / macOS 15.0.1
shared-mac: murmur3.c murmur3.h
	$(CC) -fPIC -O3 -c murmur3.c -o $(OUT_DIR)/murmur3.o
	$(CC) -shared $(OUT_DIR)/murmur3.o -o $(OUT_DIR)/libmurmur3.so

clean:
	rm -rf *.o *.so
