#!/bin/bash

# Rollkit Payload Builder Test Runner
# This script runs the integration tests in phases to help identify any issues

echo "=== Rollkit Payload Builder Test Runner ==="
echo

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo "Error: Please run this script from the rollkit-payload-builder directory"
    exit 1
fi

# Function to run a specific test with timeout
run_test() {
    local test_name="$1"
    local timeout_duration="$2"
    
    echo "Running test: $test_name"
    echo "----------------------------------------"
    
    if timeout $timeout_duration cargo test --test "$test_name" -- --nocapture; then
        echo "✅ $test_name passed"
    else
        echo "❌ $test_name failed or timed out"
        return 1
    fi
    echo
}

# Function to check compilation
check_compilation() {
    echo "Checking compilation..."
    echo "----------------------------------------"
    
    if cargo check --tests --quiet; then
        echo "✅ Compilation successful"
        return 0
    else
        echo "❌ Compilation failed"
        return 1
    fi
    echo
}

# Parse command line arguments
PHASE="all"
if [ $# -gt 0 ]; then
    PHASE="$1"
fi

case "$PHASE" in
    "compile")
        echo "Phase: Compilation Check"
        check_compilation
        ;;
    
    "basic")
        echo "Phase: Basic Integration Tests"
        check_compilation || exit 1
        run_test "integration_tests" "300s"
        ;;
    
    "engine")
        echo "Phase: Engine API Tests"
        check_compilation || exit 1
        run_test "engine_api_tests" "600s"
        ;;
    
    "quick")
        echo "Phase: Quick Test (Compilation + Basic)"
        check_compilation || exit 1
        echo "Running quick integration test..."
        cargo test --test integration_tests test_payload_attributes_validation -- --nocapture
        ;;
    
    "all")
        echo "Phase: All Tests"
        echo
        
        # Step 1: Check compilation
        check_compilation || exit 1
        
        # Step 2: Run basic integration tests
        echo "Step 2: Basic Integration Tests"
        run_test "integration_tests" "300s" || echo "⚠️  Some basic tests failed"
        
        # Step 3: Run engine API tests
        echo "Step 3: Engine API Tests"
        run_test "engine_api_tests" "600s" || echo "⚠️  Some engine API tests failed"
        
        echo "=== Test Summary ==="
        echo "All test phases completed. Check output above for details."
        ;;
    
    "help"|"-h"|"--help")
        echo "Usage: $0 [PHASE]"
        echo
        echo "Available phases:"
        echo "  compile  - Check compilation only"
        echo "  basic    - Run basic integration tests"
        echo "  engine   - Run engine API tests"
        echo "  quick    - Quick test (compilation + one basic test)"
        echo "  all      - Run all tests (default)"
        echo "  help     - Show this help message"
        echo
        echo "Examples:"
        echo "  $0 compile    # Check if tests compile"
        echo "  $0 quick      # Quick validation"
        echo "  $0 basic      # Run basic integration tests"
        echo "  $0 engine     # Run engine API tests"
        echo "  $0            # Run all tests"
        ;;
    
    *)
        echo "Unknown phase: $PHASE"
        echo "Use '$0 help' for usage information"
        exit 1
        ;;
esac 