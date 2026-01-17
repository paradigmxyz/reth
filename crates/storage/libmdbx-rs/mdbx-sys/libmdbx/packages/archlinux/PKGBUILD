# Maintainer: Леонид Юрьев (Leonid Yuriev) <leo@yuriev.ru>
# Contributor: Леонид Юрьев (Leonid Yuriev) <leo@yuriev.ru>
# Contributor: Noel Kuntze <noel.kuntze@thermi.consulting>
pkgname=libmdbx
pkgver=0.13.10
pkgrel=2
pkgdesc="One of the fastest compact key-value ACID database without WAL, which surpasses the legendary LMDB in terms of reliability, features and performance. At the end of 2024 MDBX was chosen by all modern Ethereum frontiers/nodes as a storage engine."
url="https://libmdbx.dqdkfa.ru/"
arch=('x86_64' 'i686' 'ARM' 'aarch64' 'powerpc64le')
license=('Apache-2')
depends=('glibc')
subpackages="$pkgname-dev $pkgname-doc $pkgname-dbg"
source=("$pkgname-$pkgver.tar.xz::https://libmdbx.dqdkfa.ru/release/libmdbx-amalgamated-$pkgver.tar.xz")
sha256sums=('e6c9af085390c41d101fce0a72794c77159e1e271e41077ea0fd4270b43cc56c')

build() {
	make -C "$srcdir" \
		DESTDIR="$pkgdir" prefix=/usr \
		CFLAGS="$CFLAGS -std=gnu11 -ffunction-sections -fPIC -fvisibility=hidden -pthread" \
		CXXFLAGS="$CXXFLAGS -std=gnu++20 -ffunction-sections -fPIC -fvisibility=hidden -pthread" \
		lib-shared tools
}

check() {
	echo "  Testing a storage engine is a very voluminous and complex task that requires many hours of processor time."
	echo "  Any simple tests will only verify the success of the build and create an unjustified illusion."
	echo "  Therefore, full-fledged testing of libmdbx is performed during development and releasing, but the test framework used for this purpose is not included in the amalgamated source code of libmdbx releases."
	echo "  The users are invited to use their own integration and functional tests, and if necessary to test libmdbx itself use a whole source code from the git repository."
}

package() {
	make -C "$srcdir" \
		DESTDIR="$pkgdir" prefix=/usr \
		CFLAGS="$CFLAGS -std=gnu11 -ffunction-sections -fPIC -fvisibility=hidden -pthread" \
		CXXFLAGS="$CXXFLAGS -std=gnu++20 -ffunction-sections -fPIC -fvisibility=hidden -pthread" \
		install-no-strip
}

