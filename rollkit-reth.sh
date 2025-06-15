#!/bin/bash

# Rollkit-Reth Docker Management Script
# This script provides convenient commands for managing the rollkit-reth Docker setup

set -e

COMPOSE_FILE="rollkit-reth-docker-compose.yml"
CONTAINER_NAME="rollkit-reth"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Print colored output
print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if docker-compose is available
check_docker_compose() {
    if ! command -v docker-compose &> /dev/null; then
        print_error "docker-compose is not installed or not in PATH"
        exit 1
    fi
}

# Show usage information
show_usage() {
    echo "Rollkit-Reth Docker Management Script"
    echo ""
    echo "Usage: $0 [COMMAND]"
    echo ""
    echo "Commands:"
    echo "  build       Build the rollkit-reth Docker image"
    echo "  start       Start the rollkit-reth node"
    echo "  stop        Stop the rollkit-reth node"
    echo "  restart     Restart the rollkit-reth node"
    echo "  logs        Show logs from the rollkit-reth node"
    echo "  status      Show status of the rollkit-reth node"
    echo "  clean       Stop and remove containers, networks, and volumes"
    echo "  test        Test RPC connectivity"
    echo "  shell       Open a shell in the rollkit-reth container"
    echo "  help        Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0 build && $0 start    # Build and start the node"
    echo "  $0 logs -f              # Follow logs in real-time"
    echo "  $0 test                 # Test if the node is responding"
}

# Build the Docker image
build_image() {
    print_status "Building rollkit-reth Docker image..."
    docker-compose -f "$COMPOSE_FILE" build
    print_success "Docker image built successfully"
}

# Start the rollkit-reth node
start_node() {
    print_status "Starting rollkit-reth node..."
    docker-compose -f "$COMPOSE_FILE" up -d
    print_success "Rollkit-reth node started"
    print_status "Waiting for node to be ready..."
    sleep 10
    show_status
}

# Stop the rollkit-reth node
stop_node() {
    print_status "Stopping rollkit-reth node..."
    docker-compose -f "$COMPOSE_FILE" down
    print_success "Rollkit-reth node stopped"
}

# Restart the rollkit-reth node
restart_node() {
    print_status "Restarting rollkit-reth node..."
    docker-compose -f "$COMPOSE_FILE" restart
    print_success "Rollkit-reth node restarted"
}

# Show logs
show_logs() {
    docker-compose -f "$COMPOSE_FILE" logs "${@:1}" "$CONTAINER_NAME"
}

# Show status
show_status() {
    print_status "Rollkit-reth node status:"
    docker-compose -f "$COMPOSE_FILE" ps
    
    # Check if the container is healthy
    if docker-compose -f "$COMPOSE_FILE" ps | grep -q "healthy"; then
        print_success "Node is healthy and running"
    elif docker-compose -f "$COMPOSE_FILE" ps | grep -q "Up"; then
        print_warning "Node is running but health check may be pending"
    else
        print_error "Node is not running"
    fi
}

# Clean up everything
clean_all() {
    print_warning "This will stop and remove all containers, networks, and volumes"
    read -p "Are you sure? (y/N): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        print_status "Cleaning up..."
        docker-compose -f "$COMPOSE_FILE" down -v --remove-orphans
        print_success "Cleanup completed"
    else
        print_status "Cleanup cancelled"
    fi
}

# Test RPC connectivity
test_rpc() {
    print_status "Testing RPC connectivity..."
    
    # Test HTTP RPC
    if curl -s -X POST http://localhost:8545 \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' | grep -q "result"; then
        print_success "HTTP RPC is responding"
    else
        print_error "HTTP RPC is not responding"
    fi
    
    # Test Engine API (if JWT token exists)
    if [ -f "jwttoken/jwt.hex" ]; then
        JWT_TOKEN=$(cat jwttoken/jwt.hex)
        if curl -s -X POST http://localhost:8551 \
            -H "Content-Type: application/json" \
            -H "Authorization: Bearer $JWT_TOKEN" \
            -d '{"jsonrpc":"2.0","method":"engine_exchangeCapabilities","params":[[]],"id":1}' | grep -q "result"; then
            print_success "Engine API is responding"
        else
            print_error "Engine API is not responding"
        fi
    else
        print_warning "JWT token not found, skipping Engine API test"
    fi
}

# Open shell in container
open_shell() {
    print_status "Opening shell in rollkit-reth container..."
    docker-compose -f "$COMPOSE_FILE" exec "$CONTAINER_NAME" /bin/bash
}

# Main script logic
main() {
    check_docker_compose
    
    case "${1:-help}" in
        build)
            build_image
            ;;
        start)
            start_node
            ;;
        stop)
            stop_node
            ;;
        restart)
            restart_node
            ;;
        logs)
            show_logs "${@:2}"
            ;;
        status)
            show_status
            ;;
        clean)
            clean_all
            ;;
        test)
            test_rpc
            ;;
        shell)
            open_shell
            ;;
        help|--help|-h)
            show_usage
            ;;
        *)
            print_error "Unknown command: $1"
            echo ""
            show_usage
            exit 1
            ;;
    esac
}

# Run main function with all arguments
main "$@" 