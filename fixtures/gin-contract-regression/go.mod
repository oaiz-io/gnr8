module example.com/gincontract

go 1.22

require (
	github.com/gin-gonic/gin v0.0.0
	github.com/google/uuid v0.0.0
)

replace github.com/gin-gonic/gin => ./ginstub

replace github.com/google/uuid => ./uuidstub
