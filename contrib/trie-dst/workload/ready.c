// Signal Bedrock's boot checkpoint; the controller remains alive afterwards.
#include "libvmcall.h"

int main(void) {
    vmcall_ready();
    return 0;
}
