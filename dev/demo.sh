#!/bin/sh
# Builds a small sample project (PHP + TypeScript) with git history for screenshots and smoke checks.
# Usage: sh dev/demo.sh [dir]   (default: /tmp/acme-shop)
set -eu
DIR="${1:-/tmp/acme-shop}"
rm -rf "$DIR"
mkdir -p "$DIR/src" "$DIR/public" "$DIR/web/src" "$DIR/tests"
cd "$DIR"

cat > README.md <<'MD'
# acme-shop

Sample shop backend (PHP) with a small TypeScript front end. Made for tapcode demos.
MD

cat > composer.json <<'JSON'
{
  "name": "acme/shop",
  "autoload": { "psr-4": { "Acme\\Shop\\": "src/" } },
  "require": { "php": ">=8.2" }
}
JSON

cat > .gitignore <<'TXT'
/vendor
/node_modules
/web/dist
.tapcode/
TXT

cat > src/Product.php <<'PHP'
<?php

namespace Acme\Shop;

final class Product
{
    public function __construct(
        public readonly string $sku,
        public readonly string $name,
        public readonly int $priceCents,
    ) {
    }

    public function price(): string
    {
        return number_format($this->priceCents / 100, 2);
    }
}
PHP

cat > src/Cart.php <<'PHP'
<?php

namespace Acme\Shop;

final class Cart
{
    /** @var array<string, array{product: Product, qty: int}> */
    private array $lines = [];

    public function add(Product $product, int $qty = 1): void
    {
        if ($qty < 1) {
            throw new \InvalidArgumentException('qty must be positive');
        }
        $sku = $product->sku;
        $this->lines[$sku]['product'] = $product;
        $this->lines[$sku]['qty'] = ($this->lines[$sku]['qty'] ?? 0) + $qty;
    }

    public function remove(string $sku): void
    {
        unset($this->lines[$sku]);
    }

    public function totalCents(): int
    {
        $total = 0;
        foreach ($this->lines as $line) {
            $total += $line['product']->priceCents * $line['qty'];
        }
        return $total;
    }

    public function isEmpty(): bool
    {
        return $this->lines === [];
    }
}
PHP

cat > src/Checkout.php <<'PHP'
<?php

namespace Acme\Shop;

final class Checkout
{
    public function __construct(private Cart $cart, private PaymentGateway $gateway)
    {
    }

    public function pay(): Receipt
    {
        if ($this->cart->isEmpty()) {
            throw new \LogicException('cart is empty');
        }
        $id = $this->gateway->charge($this->cart->totalCents());
        return new Receipt($id, $this->cart->totalCents());
    }
}
PHP

cat > src/PaymentGateway.php <<'PHP'
<?php

namespace Acme\Shop;

interface PaymentGateway
{
    /** Charges the amount and returns a transaction id. */
    public function charge(int $amountCents): string;
}
PHP

cat > src/Receipt.php <<'PHP'
<?php

namespace Acme\Shop;

final class Receipt
{
    public function __construct(public readonly string $transactionId, public readonly int $amountCents)
    {
    }
}
PHP

cat > public/index.php <<'PHP'
<?php

declare(strict_types=1);

use Acme\Shop\Cart;
use Acme\Shop\Product;

require __DIR__ . '/../vendor/autoload.php';

$cart = new Cart();
$cart->add(new Product('mug-01', 'Mug', 1250), 2);
$cart->add(new Product('tee-03', 'T-shirt', 2990));

header('Content-Type: application/json');
echo json_encode(['total' => $cart->totalCents()]);
PHP

cat > tests/CartTest.php <<'PHP'
<?php

use Acme\Shop\Cart;
use Acme\Shop\Product;
use PHPUnit\Framework\TestCase;

final class CartTest extends TestCase
{
    public function testTotalSumsLines(): void
    {
        $cart = new Cart();
        $cart->add(new Product('a', 'A', 100), 2);
        $cart->add(new Product('b', 'B', 50));
        self::assertSame(250, $cart->totalCents());
    }
}
PHP

cat > package.json <<'JSON'
{
  "name": "acme-shop-web",
  "private": true,
  "scripts": { "build": "tsc -p web" },
  "devDependencies": { "typescript": "^5.6" }
}
JSON

cat > web/tsconfig.json <<'JSON'
{ "compilerOptions": { "target": "ES2022", "module": "ES2022", "strict": true, "outDir": "dist" }, "include": ["src"] }
JSON

cat > web/src/api.ts <<'TS'
export interface CartTotal {
  total: number;
}

export async function fetchTotal(): Promise<CartTotal> {
  const res = await fetch('/index.php');
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}
TS

cat > web/src/cart.ts <<'TS'
import { fetchTotal } from './api';

export function formatCents(cents: number): string {
  return (cents / 100).toFixed(2);
}

export async function renderTotal(el: HTMLElement): Promise<void> {
  const { total } = await fetchTotal();
  el.textContent = `Total: $${formatCents(total)}`;
}
TS

git init -q -b main
git -c user.name="Demo" -c user.email="demo@example.com" add -A
git -c user.name="Demo" -c user.email="demo@example.com" commit -q -m "Initial shop skeleton"

# second commit: checkout flow
cat >> src/Cart.php <<'PHP'
PHP
git -c user.name="Demo" -c user.email="demo@example.com" commit -q --allow-empty -m "Add checkout and payment gateway"
git -c user.name="Demo" -c user.email="demo@example.com" commit -q --allow-empty -m "TypeScript front end: fetch and render total"

# leave uncommitted work for the Commit window
sed -i '' 's/qty must be positive/quantity must be at least 1/' src/Cart.php
cat > src/Discount.php <<'PHP'
<?php

namespace Acme\Shop;

final class Discount
{
    public function __construct(public readonly int $percent)
    {
    }

    public function apply(int $cents): int
    {
        return (int) round($cents * (100 - $this->percent) / 100);
    }
}
PHP

echo "demo project at $DIR"
