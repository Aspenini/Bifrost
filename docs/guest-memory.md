# Guest memory

`AddressSpace` uses caller-configured power-of-two pages and a maximum mapped-page count. Page-aligned mappings are zero-filled, including remapped pages. Byte access spans can cross adjacent pages. All pages and their permissions are validated before the first byte is copied.

Data reads, writes, and instruction fetches enforce separate permissions. Guard pages are inaccessible. Writable and executable permissions cannot be combined in this starter address space. Map, write, and protect operations advance per-page generations. Unmap removes the generation, and remap receives a newer one.

The implementation covers software page protection and exclusive mutation. Host mapping, reservation/commit, concurrent invalidation, and guest memory ordering are later work.
