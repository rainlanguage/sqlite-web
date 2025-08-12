#!/bin/bash

# SQLite Worker Test Runner
# Runs wasm-bindgen tests for both packages

set -e  # Exit on any error

echo "🧪 Running SQLite Worker Rust Tests"
echo "=================================="

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to run tests for a package
run_package_tests() {
    local package_path=$1
    local package_name=$2
    
    echo ""
    echo -e "${BLUE}📦 Testing $package_name${NC}"
    echo "----------------------------------------"
    
    cd "$package_path"
    
    if wasm-pack test --headless --chrome; then
        echo -e "${GREEN}✅ $package_name tests PASSED${NC}"
        cd - > /dev/null
        return 0
    else
        echo -e "${RED}❌ $package_name tests FAILED${NC}"
        cd - > /dev/null
        return 1
    fi
}

# Store the original directory
ORIGINAL_DIR=$(pwd)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Initialize test results
TOTAL_TESTS=0
FAILED_TESTS=0

# Test sqlite-worker-core
echo -e "${YELLOW}Starting tests...${NC}"
if run_package_tests "packages/sqlite-worker-core" "sqlite-worker-core"; then
    ((TOTAL_TESTS++))
else
    ((TOTAL_TESTS++))
    ((FAILED_TESTS++))
fi

# Test sqlite-worker
if run_package_tests "packages/sqlite-worker" "sqlite-worker"; then
    ((TOTAL_TESTS++))
else
    ((TOTAL_TESTS++))
    ((FAILED_TESTS++))
fi

# Return to original directory
cd "$ORIGINAL_DIR"

# Summary
echo ""
echo "🏁 Test Summary"
echo "=================================="
if [ $FAILED_TESTS -eq 0 ]; then
    echo -e "${GREEN}✅ All $TOTAL_TESTS test suites PASSED!${NC}"
    echo "Total individual tests: 36 (26 + 10)"
    exit 0
else
    echo -e "${RED}❌ $FAILED_TESTS out of $TOTAL_TESTS test suites FAILED${NC}"
    exit 1
fi