package sample

type OrderService struct {
	db *DB
}

func NewOrderService(db *DB) *OrderService {
	return &OrderService{db: db}
}

func (s *OrderService) CreateOrder(ctx context.Context) error {
	return nil
}

func helper() {}
